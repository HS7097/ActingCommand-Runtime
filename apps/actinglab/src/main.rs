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
use std::fs::{self, File, OpenOptions};
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
const REQUIRE_SESSION_DAEMON_ENV: &str = "ACTINGLAB_REQUIRE_SESSION_DAEMON";
const TRUSTED_REMOTE_TOKEN_ENV: &str = "ACTINGLAB_TRUSTED_REMOTE_TOKEN";
const TRUSTED_REMOTE_CLIENT_CERT_ENV: &str = "ACTINGLAB_TRUSTED_REMOTE_CLIENT_CERT";
const SESSION_INFO_FILE: &str = "session.json";
const SESSION_HEARTBEAT_FILE: &str = "heartbeat.json";
const SESSION_STOP_FILE: &str = "stop.request";
const SESSION_REQUESTS_DIR: &str = "requests";
const SESSION_RESPONSES_DIR: &str = "responses";
const SESSION_REQUEST_JOURNAL_FILE: &str = "request-journal.jsonl";
const SESSION_REQUEST_JOURNAL_ARCHIVE_FILE: &str = "request-journal.1.jsonl";
const SESSION_REQUEST_JOURNAL_MAX_BYTES: u64 = 1024 * 1024;
const SESSION_HEARTBEAT_STALE_MS: u64 = 2_000;
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
    require_session: bool,
    version: bool,
    // Daemon request handlers must execute local command implementations instead of
    // re-submitting work into the same resident request queue.
    inside_session_daemon: bool,
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
    adb_path: Option<String>,
    capture_backend: Option<String>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionLivenessStatus {
    Stopped,
    HeartbeatMissing,
    PidMismatch,
    Stale,
    Alive,
}

impl SessionLivenessStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::HeartbeatMissing => "heartbeat_missing",
            Self::PidMismatch => "pid_mismatch",
            Self::Stale => "stale",
            Self::Alive => "alive",
        }
    }

    fn can_accept_requests(self) -> bool {
        self == Self::Alive
    }
}

#[derive(Debug, Clone, Copy)]
struct SessionLivenessSnapshot {
    status: SessionLivenessStatus,
    heartbeat_age_ms: Option<u64>,
    heartbeat_clock_skew_ms: u64,
    pid_match: Option<bool>,
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
struct SessionRequestJournalEntry {
    request_id: String,
    command: String,
    args: Vec<String>,
    #[serde(default)]
    lease: Option<SessionCommandLease>,
    ok: bool,
    error: Option<EnvelopeError>,
    created_at_unix_ms: u64,
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
            "--require-session" => global.require_session = true,
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
    let result = enforce_session_throat_policy(&invocation)
        .and_then(|()| execute(&invocation))
        .map(|data| {
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
            "--backend <auto|adb|droidcast_raw|nemu_ipc> (alias of --capture-backend)",
            "--require-session",
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
    Ok(json!({
        "commands": command_capabilities(),
        "session_layer": session_layer_capability_contract(),
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

fn session_layer_capability_contract() -> Value {
    json!({
        "schema_version": "session.capabilities.v0.1",
        "resident_daemon": {
            "request_command": "session request capabilities",
            "status_command": "session status --diagnostics",
            "status_instance_registry_field": "diagnostics.instances",
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
                "examples": ["status", "journal", "capabilities", "devices", "session instance registry", "session instance health", "session instance keep-alive", "capture", "stream"]
            },
            "control": {
                "requires_lease": true,
                "examples": ["tap", "swipe", "long-tap", "key", "text", "session instance connect", "session instance reconnect", "session app launch", "session app stop", "session app restart", "session instance app launch", "session instance app stop", "session instance app restart", "tap-target", "navigate", "recover"]
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
                "auth_env": {
                    "token": TRUSTED_REMOTE_TOKEN_ENV,
                    "client_certificate": TRUSTED_REMOTE_CLIENT_CERT_ENV
                },
                "blocked_without_auth_code": "trusted_remote_auth_required",
                "blocked_without_encryption_code": "trusted_remote_transport_blocked"
            }
        },
        "daemon_queries": {
            "contract": "session request contract",
            "api": "session request api",
            "transport": "session request transport",
            "capabilities": "session request capabilities",
            "status": "session request status --diagnostics",
            "journal": "session request journal",
            "events": "session request events",
            "instance_registry": "session request instance registry",
            "instance_health": "session request instance health",
            "instance_keep_alive": "session request instance keep-alive"
        },
        "daemon_controls": {
            "app_lifecycle": "session request app <launch|stop|restart>",
            "instance_app_lifecycle": "session request instance app <launch|stop|restart>",
            "instance_connect": "session request instance connect",
            "instance_reconnect": "session request instance reconnect"
        },
        "request_classes": {
            "read_only": {
                "requires_lease": false,
                "examples": [
                    "status",
                    "journal",
                    "contract",
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
                    "session instance registry",
                    "session instance health",
                    "session instance keep-alive",
                    "session instance health --capture-diagnose",
                    "monitor-once"
                ]
            },
            "control": {
                "requires_lease": true,
                "examples": [
                    "lease",
                    "record",
                    "session instance connect",
                    "session instance reconnect",
                    "session app launch",
                    "session app stop",
                    "session app restart",
                    "session instance app launch",
                    "session instance app stop",
                    "session instance app restart",
                    "lab-run",
                    "package-run",
                    "operation-run",
                    "tap",
                    "swipe",
                    "long-tap",
                    "key",
                    "text",
                    "tap-target",
                    "navigate",
                    "recover"
                ]
            }
        },
        "safety": {
            "strict_session_throat_flag": "--require-session",
            "strict_session_throat_env": REQUIRE_SESSION_DAEMON_ENV,
            "strict_session_throat_failure_code": "session_daemon_required",
            "control_requests_require_matching_lease": true,
            "requests_are_serialized_by_resident_daemon": true,
            "severe_errors_fail_loud": true,
            "transient_recovery_path_must_be_logged": true
        },
        "out_of_scope": [
            "network listener",
            "TLS implementation",
            "token issuance",
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
                "status": "reserved",
                "current_scaffold": "bounded local CLI stream",
                "future_transport": "trusted bidirectional channel",
                "frame_event_schema": "session.stream.event.v0.1",
                "input_relay_requires_matching_lease": true
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
            "UI transport",
            "scheduler runtime"
        ]
    })
}

fn session_api_contract() -> Value {
    json!({
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
                "schema_version": "session.transport.v0.1"
            },
            "status_view": {
                "query": "session status --diagnostics",
                "daemon_query": "session request status --diagnostics",
                "liveness_field": "diagnostics.liveness",
                "instance_registry_field": "diagnostics.instances",
                "lease_field": "diagnostics.leases",
                "journal_field": "diagnostics.journal"
            },
            "event_view": {
                "query": "session events",
                "daemon_query": "session request events",
                "schema_version": "session.events.v0.1",
                "filters": ["--limit", "--after-unix-ms", "--after-request-id"],
                "cursor_fields": [
                    "latest_timestamp_unix_ms",
                    "next_after_unix_ms",
                    "latest_request_id",
                    "next_after_request_id"
                ],
                "cursor_error": "event_cursor_not_found"
            },
            "instance_registry_view": {
                "query": "session instance registry",
                "daemon_query": "session request instance registry",
                "schema_version": "session.instance_registry.v0.1",
                "ready_field": "instances[].validation.ready_for_device_control"
            },
            "instance_health_view": {
                "query": "session instance health [--capture-diagnose]",
                "daemon_query": "session request instance health [--capture-diagnose]",
                "status_field": "status",
                "capture_field": "capture"
            },
            "instance_keep_alive_view": {
                "query": "session instance keep-alive",
                "daemon_query": "session request instance keep-alive",
                "status_field": "status",
                "action_field": "action"
            },
            "instance_connect_view": {
                "query": "session instance connect",
                "daemon_query": "session request instance connect",
                "requires_lease": true,
                "status_field": "status",
                "action_field": "action"
            },
            "app_lifecycle_view": {
                "query": "session app <launch|stop|restart>",
                "daemon_query": "session request app <launch|stop|restart>",
                "aliases": ["session instance app <launch|stop|restart>", "session request instance app <launch|stop|restart>"],
                "requires_lease": true,
                "actions": ["launch", "stop", "restart"],
                "action_field": "action",
                "package_field": "package"
            }
        },
        "command_classes": {
            "read_only": {
                "requires_lease": false,
                "examples": [
                    "status",
                    "journal",
                    "events",
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
                    "session instance registry",
                    "session instance health",
                    "session instance keep-alive",
                    "session instance health --capture-diagnose",
                    "monitor-once"
                ]
            },
            "control": {
                "requires_lease": true,
                "examples": [
                    "lease",
                    "record",
                    "session instance connect",
                    "session instance reconnect",
                    "session app launch",
                    "session app stop",
                    "session app restart",
                    "session instance app launch",
                    "session instance app stop",
                    "session instance app restart",
                    "lab-run",
                    "package-run",
                    "operation-run",
                    "tap",
                    "swipe",
                    "long-tap",
                    "key",
                    "text",
                    "tap-target",
                    "navigate",
                    "recover"
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
    })
}

fn run_status(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    if should_route_readonly_via_session_daemon(global, &flags)? {
        return submit_readonly_session_request(global, &flags, "status", args);
    }
    require_runtime(global).map(|data| {
        json!({
            "state": "running",
            "runtime": data,
        })
    })
}

fn run_devices(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    if should_route_readonly_via_session_daemon(global, &flags)? {
        return submit_readonly_session_request(global, &flags, "devices", args);
    }
    flags.expect_positionals("devices", 0)?;
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
    if should_route_readonly_via_session_daemon(global, &flags)? {
        return submit_readonly_session_request(global, &flags, "capture", args);
    }
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
        "capture_backend_attempts": captured.attempts,
        "freshness": captured.freshness,
        "out": out.display().to_string()
    }))
}

fn run_capture_diagnose(global: &GlobalOptions, flags: &FlagArgs) -> CliOutcome<Value> {
    if should_route_readonly_via_session_daemon(global, flags)? {
        return submit_capture_diagnose_request(global, flags);
    }
    let config = read_user_config()?;
    let device_config = device_config(global, &config)?;
    let requested = device_config.capture_backend;
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

fn should_route_readonly_via_session_daemon(
    global: &GlobalOptions,
    flags: &FlagArgs,
) -> CliOutcome<bool> {
    if global.inside_session_daemon {
        return Ok(false);
    }
    if flags.bool("--local") {
        return Ok(false);
    }
    if flags.bool("--via-daemon") {
        return Ok(true);
    }
    session_daemon_info_exists(flags)
}

fn should_route_control_via_session_daemon(
    global: &GlobalOptions,
    flags: &FlagArgs,
) -> CliOutcome<bool> {
    if global.inside_session_daemon {
        return Ok(false);
    }
    if flags.bool("--via-daemon") {
        return Ok(true);
    }
    session_daemon_info_exists(flags)
}

fn enforce_session_throat_policy(invocation: &Invocation) -> CliOutcome<()> {
    if !session_throat_required(&invocation.global) || invocation.global.inside_session_daemon {
        return Ok(());
    }
    if !command_requires_session_throat(invocation) {
        return Ok(());
    }

    let flags = FlagArgs::parse(&invocation.args)?;
    if flags.bool("--local") {
        return Err(session_daemon_required_error(&invocation.command_name));
    }
    if flags.bool("--via-daemon") || session_daemon_info_exists(&flags)? {
        return Ok(());
    }
    Err(session_daemon_required_error(&invocation.command_name))
}

fn session_throat_required(global: &GlobalOptions) -> bool {
    session_throat_required_from_env(global, env_flag_enabled(REQUIRE_SESSION_DAEMON_ENV))
}

fn session_throat_required_from_env(global: &GlobalOptions, env_requires_session: bool) -> bool {
    global.require_session || env_requires_session
}

fn env_flag_enabled(name: &str) -> bool {
    env::var(name)
        .map(|value| {
            let normalized = value.trim().to_ascii_lowercase();
            !normalized.is_empty() && !matches!(normalized.as_str(), "0" | "false" | "off" | "no")
        })
        .unwrap_or(false)
}

fn command_requires_session_throat(invocation: &Invocation) -> bool {
    match invocation.command.as_slice() {
        [cmd] => matches!(
            cmd.as_str(),
            "devices"
                | "tap"
                | "swipe"
                | "long-tap"
                | "key"
                | "text"
                | "capture"
                | "detect-page"
                | "recognize"
                | "current-page"
                | "is-visible"
                | "locate"
                | "tap-target"
                | "navigate"
                | "monitor"
                | "stream"
        ),
        [group, sub] if group == "lab" => sub.as_str() == "run",
        [group, sub] if group == "package" => sub.as_str() == "run",
        [group, sub] if group == "operation" => matches!(sub.as_str(), "run" | "dry-run"),
        [group, sub] if group == "session" => {
            session_subcommand_requires_throat(sub, &invocation.args)
        }
        _ => false,
    }
}

fn session_subcommand_requires_throat(sub: &str, args: &[String]) -> bool {
    match sub {
        "capture" | "recover" | "app" => true,
        "instance" => matches!(
            args.first().map(String::as_str),
            Some("app" | "connect" | "health" | "keep-alive" | "reconnect")
        ),
        "record" | "lease" | "request" | "status" | "start" | "stop" | "cleanup" | "daemon"
        | "contract" | "api" | "transport" | "journal" | "events" => false,
        _ => false,
    }
}

fn session_daemon_required_error(command: &str) -> CliError {
    CliError::safety_blocked(
        "session_daemon_required",
        format!(
            "{command} requires an alive Session daemon when --require-session or {REQUIRE_SESSION_DAEMON_ENV} is enabled"
        ),
        &["session_layer", "running_runtime"],
    )
}

fn session_daemon_info_exists(flags: &FlagArgs) -> CliOutcome<bool> {
    let Ok(state_dir) = session_state_dir_from_flags(flags) else {
        return Ok(false);
    };
    let info_path = session_info_path(&state_dir);
    let heartbeat_path = session_heartbeat_path(&state_dir);
    let info = read_json_file::<SessionInfo>(&info_path)?;
    let heartbeat = read_json_file::<SessionHeartbeat>(&heartbeat_path)?;
    Ok(
        session_liveness_snapshot(info.as_ref(), heartbeat.as_ref(), current_unix_ms())
            .status
            .can_accept_requests(),
    )
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

fn submit_session_lease_request(
    global: &GlobalOptions,
    flags: &FlagArgs,
    args: &[String],
) -> CliOutcome<Value> {
    submit_session_command_request(
        global,
        flags,
        "lease",
        session_state_request_payload_args(args),
    )
}

fn submit_session_record_request(
    global: &GlobalOptions,
    flags: &FlagArgs,
    args: &[String],
) -> CliOutcome<Value> {
    submit_session_command_request(
        global,
        flags,
        "record",
        session_state_request_payload_args(args),
    )
}

fn session_request_payload_args(args: &[String]) -> Vec<String> {
    let mut payload = Vec::new();
    let mut index = 0usize;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--via-daemon" || arg == "--local" {
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

fn session_state_request_payload_args(args: &[String]) -> Vec<String> {
    let mut payload = Vec::new();
    let mut index = 0usize;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--via-daemon" || arg == "--local" {
            index += 1;
            continue;
        }
        if ["--state-dir", "--request-timeout-ms"].contains(&arg.as_str()) {
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

    let selected = create_capture_backend(device_config.capture_backend_config())
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
        let selected = match create_capture_backend(
            device_config
                .capture_backend_config()
                .with_requested(choice),
        ) {
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

fn instance_health_status(capture_status: Option<CaptureFreshProbeStatus>) -> &'static str {
    match capture_status {
        Some(CaptureFreshProbeStatus::Fresh) => "healthy",
        Some(CaptureFreshProbeStatus::StaleSuspected) => "capture_stale_suspected",
        Some(CaptureFreshProbeStatus::Unavailable) => "capture_unavailable",
        None => "device_connected",
    }
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

fn parse_optional_u64(flags: &FlagArgs, name: &str) -> CliOutcome<Option<u64>> {
    let Some(value) = flags.optional(name).filter(|value| value != "true") else {
        return Ok(None);
    };
    value
        .parse::<u64>()
        .map(Some)
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
    if should_route_control_via_session_daemon(global, &flags)? {
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
    if should_route_control_via_session_daemon(global, &flags)? {
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

    fn run(&self, backend: &mut MaaTouchBackend) -> actingcommand_device::DeviceResult<()> {
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

fn stream_input_relay_requested(flags: &FlagArgs) -> bool {
    flags.optional("--input-relay").is_some()
        || flags.optional("--interactive-input").is_some()
        || !flags.values("--input-event").is_empty()
        || !flags.values("--relay-event").is_empty()
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
    let operation = actions
        .iter()
        .try_for_each(|action| action.run(&mut backend));
    let close = backend.close();
    combine_operation_and_close(operation, close)
        .map_err(|err| CliError::device(err.to_string()))?;
    Ok(json!({
        "status": "sent",
        "backend": "maatouch",
        "control_mode": "stream_input_relay",
        "serial": serial,
        "device_state": device.state,
        "screen_size": device.screen_size,
        "handshake": handshake.map(handshake_json),
        "action_count": actions.len(),
        "action": action_values.first().cloned(),
        "actions": action_values
    }))
}

fn run_recognize(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    if should_route_readonly_via_session_daemon(global, &flags)? {
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
    if should_route_readonly_via_session_daemon(global, &flags)? {
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
    if should_route_readonly_via_session_daemon(global, &flags)? {
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
    if should_route_readonly_via_session_daemon(global, &flags)? {
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
    if should_route_readonly_via_session_daemon(global, &flags)? {
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
    if should_route_control_via_session_daemon(global, &flags)? {
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
    if should_route_control_via_session_daemon(global, &flags)? {
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
    if should_route_control_via_session_daemon(global, &flags)? {
        return submit_control_session_request(global, &flags, "recover", args);
    }
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
    Ok(json!({
        "status": "planned",
        "mode": "stale_capture_recovery",
        "executed": false,
        "click_allowed": false,
        "app_restart_executed": false,
        "requested_backend": requested.as_str(),
        "fresh_delay_ms": fresh_delay.as_millis(),
        "diagnosis": {
            "command": format!(
                "capture diagnose --capture-backend {} --fresh-delay-ms {}",
                requested.as_str(),
                fresh_delay.as_millis()
            ),
            "read_only": true,
            "reason": "verify fresh frames before treating an unchanged screen as a game freeze"
        },
        "recovery": capture_diagnosis_recovery_json(
            CaptureFreshProbeStatus::StaleSuspected,
            requested,
        ),
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
                "type": "device_health",
                "command": "session instance health",
                "read_only": true
            },
            {
                "order": 5,
                "type": "app_restart",
                "command": "session app restart",
                "requires_lease": true,
                "heavy_recovery": true,
                "reason": "last resort after capture-backend recovery checks fail"
            }
        ],
        "safety_gate": "diagnose_capture_backend_before_restart",
        "next": "run capture diagnose with the effective backend selection; only restart the app if lighter capture-backend recovery cannot restore fresh frames"
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
    if flags.bool("--recover") {
        if should_route_control_via_session_daemon(global, &flags)? {
            return submit_monitor_session_request(global, &flags, args);
        }
    } else if should_route_readonly_via_session_daemon(global, &flags)? {
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
    let relay_actions = StreamInputRelayAction::parse_many(&flags)?;
    if !relay_actions.is_empty() && should_route_control_via_session_daemon(global, &flags)? {
        return submit_control_session_request(global, &flags, "stream", args);
    }
    if relay_actions.is_empty() && should_route_readonly_via_session_daemon(global, &flags)? {
        return submit_readonly_session_request(global, &flags, "stream", args);
    }
    let config = read_user_config()?;
    let instance_id = resolve_instance_id_for_flags(global, &config, &flags)?;
    let max_frames = parse_optional_usize(&flags, "--max-frames", 1)?;
    if max_frames == 0 || max_frames > 60 {
        return Err(CliError::usage("--max-frames must be between 1 and 60"));
    }
    let interval = parse_optional_duration_ms(&flags, "--interval-ms", 250)?;
    let fresh_delay = parse_optional_duration_ms(&flags, "--fresh-delay-ms", 160)?;
    let dry_run = global.dry_run || flags.bool("--dry-run");
    let input_relay = if relay_actions.is_empty() {
        json!({
            "status": "disabled",
            "reason": "no input relay action requested"
        })
    } else {
        run_stream_input_relay(global, &config, &relay_actions, dry_run)?
    };
    let frames = if dry_run {
        stream_dry_run_frames(max_frames)
    } else {
        let device_config = device_config_for_instance(global, &config, Some(&instance_id))?;
        stream_capture_frames(
            global,
            &flags,
            &device_config,
            max_frames,
            interval,
            fresh_delay,
        )?
    };
    let contract = stream_contract_json(
        max_frames,
        interval,
        fresh_delay,
        flags.bool("--require-fresh"),
        relay_actions.len(),
        dry_run,
    );
    let stream_id = format!("stream-{}-{}", current_unix_ms(), std::process::id());
    let events = stream_events_json(&stream_id, &frames, &input_relay);
    Ok(json!({
        "stream_id": stream_id,
        "mode": "bounded_stream",
        "instance": instance_id,
        "transport": "local_cli",
        "max_frames": max_frames,
        "interval_ms": interval.as_millis(),
        "capture": {
            "require_fresh": flags.bool("--require-fresh"),
            "dry_run": dry_run
        },
        "trusted_channel": {
            "status": "reserved",
            "reason": "local CLI bounded stream scaffold only"
        },
        "contract": contract,
        "input_relay": input_relay,
        "events": events,
        "frames": frames
    }))
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
        "stream_kind": "bounded_cli_frame_sequence",
        "frame_delivery": "json_array",
        "event_schema_version": "session.stream.event.v0.1",
        "event_fields": ["schema_version", "stream_id", "event_index", "type"],
        "input_relay": {
            "supported": true,
            "requested": input_event_count > 0,
            "event_count": input_event_count,
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
            "trusted_remote_channel": "reserved"
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

fn stream_dry_run_frames(max_frames: usize) -> Vec<Value> {
    (0..max_frames)
        .map(|index| {
            json!({
                "index": index,
                "captured": false,
                "mode": "dry_run"
            })
        })
        .collect()
}

fn stream_capture_frames(
    _global: &GlobalOptions,
    flags: &FlagArgs,
    device_config: &DeviceRuntimeConfig,
    max_frames: usize,
    interval: Duration,
    fresh_delay: Duration,
) -> CliOutcome<Vec<Value>> {
    let requested = device_config.capture_backend;
    let mut frames = Vec::with_capacity(max_frames);
    for index in 0..max_frames {
        let captured = capture_for_command(
            device_config,
            requested,
            flags.bool("--require-fresh"),
            fresh_delay,
        )?;
        frames.push(json!({
            "index": index,
            "captured": true,
            "captured_at_unix_ms": current_unix_ms(),
            "frame": capture_frame_summary_json(&captured.frame),
            "freshness": captured.freshness,
            "capture_backend_attempts": captured.attempts
        }));
        if index + 1 < max_frames && !interval.is_zero() {
            thread::sleep(interval);
        }
    }
    Ok(frames)
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

fn submit_session_instance_request(
    global: &GlobalOptions,
    flags: &FlagArgs,
    args: &[String],
) -> CliOutcome<Value> {
    match args.first().map(String::as_str) {
        Some("reconnect") => submit_control_session_request(global, flags, "instance", args),
        _ => submit_readonly_session_request(global, flags, "instance", args),
    }
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
        "run" => {
            let flags = FlagArgs::parse(args)?;
            if should_route_control_via_session_daemon(global, &flags)? {
                submit_control_session_request(global, &flags, "lab_run", args)
            } else {
                lab_run::run_lab_run(global, args)
            }
        }
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
        "status" => run_session_status(global, args),
        "lease" => run_lab_lease(global, args),
        "preempt" => run_lab_preempt(global, args),
        "release" => run_lab_release(global, args),
        _ => Err(CliError::usage(format!("unknown lab command: {sub}"))),
    }
}

fn run_lab_lease(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    match args.first().map(String::as_str) {
        Some("acquire" | "status") => run_session_lease_inner(global, args, None),
        _ => {
            let mut lease_args = vec!["acquire".to_string()];
            lease_args.extend_from_slice(args);
            run_session_lease_inner(global, &lease_args, None)
        }
    }
}

fn run_lab_preempt(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let mut lease_args = vec!["preempt".to_string()];
    lease_args.extend_from_slice(args);
    run_session_lease_inner(global, &lease_args, None)
}

fn run_lab_release(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let mut lease_args = vec!["release".to_string()];
    lease_args.extend_from_slice(args);
    run_session_lease_inner(global, &lease_args, None)
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
            if should_route_control_via_session_daemon(global, &flags)? {
                return submit_control_session_request(global, &flags, "package_run", args);
            }
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
        "run" => {
            if should_route_control_via_session_daemon(global, &flags)? {
                submit_control_session_request(global, &flags, "operation_run", args)
            } else {
                Err(CliError::safety_blocked(
                    "lab_lease_required",
                    "operation run requires navigation_only operations and an exclusive_drain LabLease",
                    &["lab_lease", "exclusive_drain"],
                ))
            }
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
        "start" => run_session_start(args),
        "stop" => run_session_stop(args),
        "cleanup" => run_session_cleanup(global, args),
        "daemon" => run_session_daemon(args),
        "request" => run_session_request(global, args),
        "contract" => run_session_contract(global, args),
        "api" => run_session_api(global, args),
        "transport" => run_session_transport(global, args),
        "journal" => run_session_journal(global, args),
        "events" => run_session_events(global, args),
        "instance" => run_session_instance(global, args),
        "app" => run_session_app(global, args),
        "capture" => run_capture(global, args),
        "recover" => run_session_recover(global, args),
        "lease" => run_session_lease(global, args),
        "record" => run_session_record(global, args),
        _ => Err(CliError::usage(format!("unknown session command: {sub}"))),
    }
}

fn run_session_contract(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    if should_route_readonly_via_session_daemon(global, &flags)? {
        return submit_readonly_session_request(global, &flags, "contract", args);
    }
    flags.expect_positionals("session contract", 0)?;
    Ok(session_access_contract())
}

fn run_session_api(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    if should_route_readonly_via_session_daemon(global, &flags)? {
        return submit_readonly_session_request(global, &flags, "api", args);
    }
    flags.expect_positionals("session api", 0)?;
    Ok(session_api_contract())
}

fn run_session_transport(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    if should_route_readonly_via_session_daemon(global, &flags)? {
        return submit_readonly_session_request(global, &flags, "transport", args);
    }
    flags.expect_positionals("session transport", 0)?;
    Ok(session_transport_contract())
}

fn run_session_events(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    if should_route_readonly_via_session_daemon(global, &flags)? {
        return submit_readonly_session_request(global, &flags, "events", args);
    }
    flags.expect_positionals("session events", 0)?;
    let state_dir = session_state_dir_from_flags(&flags)?;
    let limit = parse_optional_usize(&flags, "--limit", 20)?;
    let after_unix_ms = parse_optional_u64(&flags, "--after-unix-ms")?;
    let after_request_id = parse_optional_string_value(&flags, "--after-request-id")?;
    session_events_payload(
        &state_dir,
        limit,
        after_unix_ms,
        after_request_id.as_deref(),
    )
}

fn session_events_payload(
    state_dir: &Path,
    limit: usize,
    after_unix_ms: Option<u64>,
    after_request_id: Option<&str>,
) -> CliOutcome<Value> {
    if limit == 0 || limit > 1_000 {
        return Err(CliError::usage("--limit must be between 1 and 1000"));
    }
    let read_limit = if after_unix_ms.is_some() || after_request_id.is_some() {
        1_000
    } else {
        limit
    };
    let mut entries = read_session_request_journal(state_dir, read_limit)?;
    if let Some(cursor_request_id) = after_request_id {
        let Some(position) = entries
            .iter()
            .position(|entry| entry.request_id == cursor_request_id)
        else {
            return Err(CliError::new(
                ErrorKind::UsageValidation,
                "event_cursor_not_found",
                format!(
                    "request cursor '{cursor_request_id}' was not found in the recent request journal"
                ),
                &["request_journal"],
            ));
        };
        entries.drain(0..=position);
    }
    let mut entries = entries
        .into_iter()
        .filter(|entry| {
            after_unix_ms
                .map(|after| entry.completed_at_unix_ms > after)
                .unwrap_or(true)
        })
        .collect::<Vec<_>>();
    if entries.len() > limit {
        let keep_from = entries.len() - limit;
        entries.drain(0..keep_from);
    }
    let events = entries
        .iter()
        .map(session_request_event_json)
        .collect::<Vec<_>>();
    let latest_timestamp_unix_ms = entries.iter().map(|entry| entry.completed_at_unix_ms).max();
    let latest_request_id = entries.last().map(|entry| entry.request_id.as_str());
    Ok(json!({
        "schema_version": "session.events.v0.1",
        "state_dir": state_dir.display().to_string(),
        "source": "request_journal",
        "journal": session_request_journal_path(state_dir).display().to_string(),
        "limit": limit,
        "after_unix_ms": after_unix_ms,
        "after_request_id": after_request_id,
        "event_count": events.len(),
        "cursor": {
            "latest_timestamp_unix_ms": latest_timestamp_unix_ms,
            "next_after_unix_ms": latest_timestamp_unix_ms,
            "latest_request_id": latest_request_id,
            "next_after_request_id": latest_request_id
        },
        "events": events
    }))
}

fn session_request_event_json(entry: &SessionRequestJournalEntry) -> Value {
    let status = if entry.ok { "completed" } else { "failed" };
    let event_type = if entry.ok {
        "session.request.completed"
    } else {
        "session.request.failed"
    };
    let lease = entry.lease.as_ref().map(|lease| {
        json!({
            "holder": &lease.holder,
            "lease_id": &lease.lease_id
        })
    });
    let error = entry.error.as_ref().map(|error| {
        json!({
            "code": &error.code,
            "message": &error.message,
            "blocked_by": &error.blocked_by
        })
    });
    json!({
        "schema_version": "session.event.v0.1",
        "type": event_type,
        "timestamp_unix_ms": entry.completed_at_unix_ms,
        "request_id": &entry.request_id,
        "command": &entry.command,
        "status": status,
        "ok": entry.ok,
        "args_count": entry.args.len(),
        "lease": lease,
        "error": error,
        "timing": {
            "created_at_unix_ms": entry.created_at_unix_ms,
            "started_at_unix_ms": entry.started_at_unix_ms,
            "completed_at_unix_ms": entry.completed_at_unix_ms,
            "queue_wait_ms": entry.started_at_unix_ms.saturating_sub(entry.created_at_unix_ms),
            "duration_ms": entry.completed_at_unix_ms.saturating_sub(entry.started_at_unix_ms)
        }
    })
}

fn run_session_journal(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    if should_route_readonly_via_session_daemon(global, &flags)? {
        return submit_readonly_session_request(global, &flags, "journal", args);
    }
    flags.expect_positionals("session journal", 0)?;
    let state_dir = session_state_dir_from_flags(&flags)?;
    let limit = parse_optional_usize(&flags, "--limit", 20)?;
    session_journal_payload(&state_dir, limit)
}

fn session_journal_payload(state_dir: &Path, limit: usize) -> CliOutcome<Value> {
    if limit == 0 || limit > 1_000 {
        return Err(CliError::usage("--limit must be between 1 and 1000"));
    }
    let entries = read_session_request_journal(state_dir, limit)?;
    Ok(json!({
        "state_dir": state_dir.display().to_string(),
        "journal": session_request_journal_path(state_dir).display().to_string(),
        "limit": limit,
        "entries": entries
    }))
}

fn run_session_status(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    if should_route_readonly_via_session_daemon(global, &flags)? {
        return submit_readonly_session_request(global, &flags, "status", args);
    }
    flags.expect_positionals("session status", 0)?;
    let state_dir = session_state_dir_from_flags(&flags)?;
    let diagnostics = flags.bool("--diagnostics");
    let config = if diagnostics {
        Some(read_user_config()?)
    } else {
        None
    };
    session_status_payload_with_config(&state_dir, diagnostics, config.as_ref())
}

#[cfg(test)]
fn session_status_payload(state_dir: &Path, diagnostics: bool) -> CliOutcome<Value> {
    session_status_payload_with_config(state_dir, diagnostics, None)
}

fn session_status_payload_with_config(
    state_dir: &Path,
    diagnostics: bool,
    config: Option<&UserConfig>,
) -> CliOutcome<Value> {
    let info_path = session_info_path(state_dir);
    let heartbeat_path = session_heartbeat_path(state_dir);
    let info = read_json_file::<SessionInfo>(&info_path)?;
    let heartbeat = read_json_file::<SessionHeartbeat>(&heartbeat_path)?;
    let diagnostics_payload = if diagnostics {
        Some(session_status_diagnostics(
            state_dir,
            info.as_ref(),
            heartbeat.as_ref(),
            current_unix_ms(),
            config,
        )?)
    } else {
        None
    };
    let mut status = json!({
        "state_dir": state_dir.display().to_string(),
        "running": info.is_some(),
        "info": info,
        "heartbeat": heartbeat
    });
    if let Some(diagnostics) = diagnostics_payload {
        status["diagnostics"] = diagnostics;
    }
    Ok(status)
}

fn session_status_diagnostics(
    state_dir: &Path,
    info: Option<&SessionInfo>,
    heartbeat: Option<&SessionHeartbeat>,
    now_ms: u64,
    config: Option<&UserConfig>,
) -> CliOutcome<Value> {
    let recent_entries = read_session_request_journal(state_dir, 5)?;
    let last_entry = recent_entries.last();
    let last_error = recent_entries.iter().rev().find(|entry| !entry.ok);
    let liveness = session_liveness_snapshot(info, heartbeat, now_ms);
    Ok(json!({
        "liveness": session_liveness_diagnostics(info, heartbeat, now_ms),
        "recommended_actions": session_liveness_recommended_actions(state_dir, liveness.status),
        "paths": {
            "info": session_info_path(state_dir).display().to_string(),
            "heartbeat": session_heartbeat_path(state_dir).display().to_string(),
            "requests": session_requests_dir(state_dir).display().to_string(),
            "responses": session_responses_dir(state_dir).display().to_string(),
            "journal": session_request_journal_path(state_dir).display().to_string()
        },
        "queues": {
            "pending_requests": count_files_with_extension(&session_requests_dir(state_dir), "json")?,
            "pending_responses": count_files_with_extension(&session_responses_dir(state_dir), "json")?
        },
        "instances": session_instance_registry_diagnostics(config),
        "leases": session_lease_diagnostics(state_dir)?,
        "journal": {
            "exists": session_request_journal_path(state_dir).exists(),
            "path": session_request_journal_path(state_dir).display().to_string(),
            "bytes": file_size_if_exists(&session_request_journal_path(state_dir))?,
            "total_entries": count_session_request_journal_entries(state_dir)?,
            "recent_limit": 5,
            "recent_count": recent_entries.len(),
            "last_entry": last_entry,
            "last_error": last_error,
            "retention": {
                "max_bytes": SESSION_REQUEST_JOURNAL_MAX_BYTES,
                "archive_count": 1,
                "active_rotation": "size"
            },
            "archive": {
                "path": session_request_journal_archive_path(state_dir).display().to_string(),
                "exists": session_request_journal_archive_path(state_dir).exists(),
                "bytes": file_size_if_exists(&session_request_journal_archive_path(state_dir))?
            }
        }
    }))
}

fn session_instance_registry_diagnostics(config: Option<&UserConfig>) -> Value {
    let Some(config) = config else {
        return json!({
            "available": false,
            "count": 0,
            "instances": []
        });
    };
    let instances = config
        .instances
        .iter()
        .map(|(id, instance)| {
            json!({
                "id": id,
                "serial": instance.serial,
                "game": instance.game,
                "server": instance.server,
                "package": instance.package,
                "adb_path": instance.adb_path,
                "capture_backend": instance.capture_backend,
                "serial_configured": instance.serial.is_some(),
                "game_configured": instance.game.is_some(),
                "server_configured": instance.server.is_some(),
                "package_configured": instance.package.is_some(),
                "adb_path_configured": instance.adb_path.is_some(),
                "capture_backend_configured": instance.capture_backend.is_some()
            })
        })
        .collect::<Vec<_>>();
    json!({
        "available": true,
        "count": instances.len(),
        "instances": instances
    })
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
        "recommended_fields": ["package", "adb_path", "capture_backend"],
        "capture_backends": ["auto", "adb", "droidcast_raw", "nemu_ipc"],
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
        "configured": {
            "serial": instance.serial.is_some(),
            "game": instance.game.is_some(),
            "server": instance.server.is_some(),
            "package": instance.package.is_some(),
            "adb_path": instance.adb_path.is_some(),
            "capture_backend": instance.capture_backend.is_some()
        },
        "effective": {
            "capture_backend": effective_capture_backend,
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
    ]
    .into_iter()
    .filter_map(|(field, missing)| missing.then_some(field))
    .collect()
}

fn session_liveness_recommended_actions(
    state_dir: &Path,
    status: SessionLivenessStatus,
) -> Vec<Value> {
    let state_dir = state_dir.display().to_string();
    match status {
        SessionLivenessStatus::Alive => Vec::new(),
        SessionLivenessStatus::Stopped => vec![session_recommended_action(
            1,
            "start_session",
            "Start the resident session daemon.",
            vec!["session", "start", "--state-dir", &state_dir],
        )],
        SessionLivenessStatus::HeartbeatMissing
        | SessionLivenessStatus::PidMismatch
        | SessionLivenessStatus::Stale => vec![
            session_recommended_action(
                1,
                "inspect_stale_cleanup",
                "Inspect stale local session cleanup before deleting files.",
                vec![
                    "session",
                    "cleanup",
                    "--stale",
                    "--dry-run",
                    "--state-dir",
                    &state_dir,
                ],
            ),
            session_recommended_action(
                2,
                "cleanup_stale_session",
                "Remove stale local session files while preserving request journals.",
                vec!["session", "cleanup", "--stale", "--state-dir", &state_dir],
            ),
            session_recommended_action(
                3,
                "start_session",
                "Start a fresh resident session daemon after stale cleanup.",
                vec!["session", "start", "--state-dir", &state_dir],
            ),
        ],
    }
}

fn session_recommended_action(priority: u8, action: &str, reason: &str, args: Vec<&str>) -> Value {
    json!({
        "priority": priority,
        "action": action,
        "reason": reason,
        "args": args,
        "command": args.join(" ")
    })
}

fn session_liveness_diagnostics(
    info: Option<&SessionInfo>,
    heartbeat: Option<&SessionHeartbeat>,
    now_ms: u64,
) -> Value {
    let snapshot = session_liveness_snapshot(info, heartbeat, now_ms);
    json!({
        "status": snapshot.status.as_str(),
        "info_present": info.is_some(),
        "heartbeat_present": heartbeat.is_some(),
        "daemon_pid": info.map(|value| value.pid),
        "heartbeat_pid": heartbeat.map(|value| value.pid),
        "pid_match": snapshot.pid_match,
        "heartbeat_state": heartbeat.map(|value| value.state.as_str()),
        "heartbeat_updated_at_unix_ms": heartbeat.map(|value| value.updated_at_unix_ms),
        "heartbeat_age_ms": snapshot.heartbeat_age_ms,
        "heartbeat_clock_skew_ms": snapshot.heartbeat_clock_skew_ms,
        "stale_after_ms": SESSION_HEARTBEAT_STALE_MS,
        "can_accept_requests": snapshot.status.can_accept_requests()
    })
}

fn session_liveness_snapshot(
    info: Option<&SessionInfo>,
    heartbeat: Option<&SessionHeartbeat>,
    now_ms: u64,
) -> SessionLivenessSnapshot {
    let heartbeat_age_ms = heartbeat.map(|value| now_ms.saturating_sub(value.updated_at_unix_ms));
    let heartbeat_clock_skew_ms = heartbeat
        .and_then(|value| value.updated_at_unix_ms.checked_sub(now_ms))
        .unwrap_or(0);
    let pid_match = match (info, heartbeat) {
        (Some(info), Some(heartbeat)) => Some(info.pid == heartbeat.pid),
        _ => None,
    };
    let status = match (info, heartbeat, pid_match, heartbeat_age_ms) {
        (None, _, _, _) => SessionLivenessStatus::Stopped,
        (Some(_), None, _, _) => SessionLivenessStatus::HeartbeatMissing,
        (Some(_), Some(_), Some(false), _) => SessionLivenessStatus::PidMismatch,
        (Some(_), Some(_), _, Some(age)) if age > SESSION_HEARTBEAT_STALE_MS => {
            SessionLivenessStatus::Stale
        }
        (Some(_), Some(_), _, _) => SessionLivenessStatus::Alive,
    };
    SessionLivenessSnapshot {
        status,
        heartbeat_age_ms,
        heartbeat_clock_skew_ms,
        pid_match,
    }
}

fn session_lease_diagnostics(state_dir: &Path) -> CliOutcome<Value> {
    let mut paths = session_lease_paths(state_dir)?;
    paths.sort();
    let mut released_during_read_count = 0usize;
    let mut leases = Vec::new();
    for path in paths {
        let Some(lease) = read_json_file::<SessionLease>(&path)? else {
            released_during_read_count += 1;
            continue;
        };
        leases.push(json!({
            "instance": lease.instance,
            "holder": lease.holder,
            "lease_id": lease.lease_id,
            "acquired_at_unix_ms": lease.acquired_at_unix_ms,
            "updated_at_unix_ms": lease.updated_at_unix_ms,
            "preempted": lease.preempted,
            "previous": lease.previous,
            "path": path.display().to_string()
        }));
    }
    Ok(json!({
        "path": state_dir.display().to_string(),
        "active_count": leases.len(),
        "released_during_read_count": released_during_read_count,
        "leases": leases
    }))
}

fn session_lease_paths(state_dir: &Path) -> CliOutcome<Vec<PathBuf>> {
    if !state_dir.exists() {
        return Ok(Vec::new());
    }
    let entries = fs::read_dir(state_dir).map_err(|err| {
        CliError::runtime_not_running(format!(
            "failed to read session state directory {}: {err}",
            state_dir.display()
        ))
    })?;
    let mut paths = Vec::new();
    for entry in entries {
        let path = entry
            .map_err(|err| {
                CliError::runtime_not_running(format!(
                    "failed to read session state directory entry {}: {err}",
                    state_dir.display()
                ))
            })?
            .path();
        let Some(file_name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if path.extension().and_then(|value| value.to_str()) == Some("json")
            && file_name.starts_with("lease-")
        {
            paths.push(path);
        }
    }
    Ok(paths)
}

fn run_session_request(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let command = args
        .first()
        .map(String::as_str)
        .ok_or_else(|| {
            CliError::usage(
                "session request requires status, journal, events, contract, api, transport, capabilities, devices, lease, record, capture, capture-diagnose, stream, recognize, detect-page, current-page, is-visible, locate, monitor, monitor-once, instance, app, lab-run, package-run, operation-run, tap, swipe, long-tap, key, text, tap-target, navigate, or recover",
            )
        })?;
    let flags = FlagArgs::parse(&args[1..])?;
    match command {
        "status" => submit_readonly_session_request(global, &flags, "status", &args[1..]),
        "journal" => submit_readonly_session_request(global, &flags, "journal", &args[1..]),
        "events" => submit_readonly_session_request(global, &flags, "events", &args[1..]),
        "contract" => submit_readonly_session_request(global, &flags, "contract", &args[1..]),
        "api" => submit_readonly_session_request(global, &flags, "api", &args[1..]),
        "transport" => submit_readonly_session_request(global, &flags, "transport", &args[1..]),
        "capabilities" => {
            submit_readonly_session_request(global, &flags, "capabilities", &args[1..])
        }
        "devices" => submit_readonly_session_request(global, &flags, "devices", &args[1..]),
        "lease" => submit_session_lease_request(global, &flags, &args[1..]),
        "record" => submit_session_record_request(global, &flags, &args[1..]),
        "capture" => submit_readonly_session_request(global, &flags, "capture", &args[1..]),
        "capture-diagnose" => {
            let mut request_args = vec!["diagnose".to_string()];
            push_optional_flag_value(&mut request_args, &flags, "--fresh-delay-ms");
            submit_session_command_request(global, &flags, "capture_diagnose", request_args)
        }
        "stream" => {
            if stream_input_relay_requested(&flags) {
                submit_control_session_request(global, &flags, "stream", &args[1..])
            } else {
                submit_readonly_session_request(global, &flags, "stream", &args[1..])
            }
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
        "instance" => submit_session_instance_request(global, &flags, &args[1..]),
        "app" => submit_control_session_request(global, &flags, "app", &args[1..]),
        "lab-run" => submit_control_session_request(global, &flags, "lab_run", &args[1..]),
        "package-run" => submit_control_session_request(global, &flags, "package_run", &args[1..]),
        "operation-run" => {
            submit_control_session_request(global, &flags, "operation_run", &args[1..])
        }
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
    let heartbeat_path = session_heartbeat_path(&state_dir);
    let heartbeat = read_json_file::<SessionHeartbeat>(&heartbeat_path)?;
    let liveness = session_liveness_snapshot(Some(&info), heartbeat.as_ref(), current_unix_ms());
    if !liveness.status.can_accept_requests() {
        return Err(CliError::runtime_not_running(format!(
            "session daemon is not accepting requests; liveness status={}, heartbeat_age_ms={}, stale_after_ms={}, state_dir={}",
            liveness.status.as_str(),
            optional_u64_text(liveness.heartbeat_age_ms),
            SESSION_HEARTBEAT_STALE_MS,
            state_dir.display()
        )));
    }
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
            inside_session_daemon: true,
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
    let heartbeat_path = session_heartbeat_path(&state_dir);
    if let Some(info) = read_json_file::<SessionInfo>(&info_path)? {
        let heartbeat = read_json_file::<SessionHeartbeat>(&heartbeat_path)?;
        let now_ms = current_unix_ms();
        let liveness = session_liveness_snapshot(Some(&info), heartbeat.as_ref(), now_ms);
        if liveness.status.can_accept_requests() {
            let liveness_json =
                session_liveness_diagnostics(Some(&info), heartbeat.as_ref(), now_ms);
            return Ok(json!({
                "status": "already_running",
                "state_dir": state_dir.display().to_string(),
                "info": info,
                "heartbeat": heartbeat,
                "liveness": liveness_json
            }));
        }
        return Err(CliError::runtime_not_running(format!(
            "session state exists but daemon is not accepting requests; liveness status={}, heartbeat_age_ms={}, stale_after_ms={}, state_dir={}",
            liveness.status.as_str(),
            optional_u64_text(liveness.heartbeat_age_ms),
            SESSION_HEARTBEAT_STALE_MS,
            state_dir.display()
        )));
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
    let mut last_liveness_status = "missing_info".to_string();
    while started.elapsed() < Duration::from_secs(2) {
        let info = read_json_file::<SessionInfo>(&info_path)?;
        let heartbeat = read_json_file::<SessionHeartbeat>(&heartbeat_path)?;
        if let Some(info) = info {
            let now_ms = current_unix_ms();
            let liveness = session_liveness_snapshot(Some(&info), heartbeat.as_ref(), now_ms);
            last_liveness_status = liveness.status.as_str().to_string();
            if !liveness.status.can_accept_requests() {
                thread::sleep(Duration::from_millis(100));
                continue;
            }
            let liveness_json =
                session_liveness_diagnostics(Some(&info), heartbeat.as_ref(), now_ms);
            return Ok(json!({
                "status": "started",
                "state_dir": state_dir.display().to_string(),
                "spawned_pid": info.pid,
                "info": info,
                "heartbeat": heartbeat,
                "liveness": liveness_json
            }));
        }
        thread::sleep(Duration::from_millis(100));
    }

    Err(CliError::runtime_not_running(format!(
        "session daemon did not become alive within startup deadline; last_liveness_status={}; info={}; heartbeat={}",
        last_liveness_status,
        info_path.display(),
        heartbeat_path.display()
    )))
}

fn run_session_stop(args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    let state_dir = session_state_dir_from_flags(&flags)?;
    let info_path = session_info_path(&state_dir);
    let info = read_json_file::<SessionInfo>(&info_path)?;
    let Some(info) = info else {
        return Ok(json!({
            "status": "not_running",
            "state_dir": state_dir.display().to_string()
        }));
    };
    let heartbeat_path = session_heartbeat_path(&state_dir);
    let heartbeat = read_json_file::<SessionHeartbeat>(&heartbeat_path)?;
    let now_ms = current_unix_ms();
    let liveness = session_liveness_snapshot(Some(&info), heartbeat.as_ref(), now_ms);
    let liveness_json = session_liveness_diagnostics(Some(&info), heartbeat.as_ref(), now_ms);
    if !liveness.status.can_accept_requests() {
        return Err(CliError::runtime_not_running(format!(
            "session stop refused because daemon is not accepting requests; liveness status={}, heartbeat_age_ms={}, stale_after_ms={}, state_dir={}",
            liveness.status.as_str(),
            optional_u64_text(liveness.heartbeat_age_ms),
            SESSION_HEARTBEAT_STALE_MS,
            state_dir.display()
        )));
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
        "heartbeat": heartbeat,
        "liveness": liveness_json,
        "info": info
    }))
}

fn run_session_cleanup(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    flags.expect_positionals("session cleanup", 0)?;
    if !flags.bool("--stale") {
        return Err(CliError::usage(
            "session cleanup requires --stale to make local stale-state cleanup explicit",
        ));
    }
    let dry_run = global.dry_run || flags.bool("--dry-run");
    let state_dir = session_state_dir_from_flags(&flags)?;
    let info = read_json_file::<SessionInfo>(&session_info_path(&state_dir))?;
    let heartbeat = read_json_file::<SessionHeartbeat>(&session_heartbeat_path(&state_dir))?;
    let now_ms = current_unix_ms();
    let liveness = session_liveness_snapshot(info.as_ref(), heartbeat.as_ref(), now_ms);
    let liveness_json = session_liveness_diagnostics(info.as_ref(), heartbeat.as_ref(), now_ms);
    if liveness.status.can_accept_requests() {
        return Err(CliError::runtime_not_running(format!(
            "session cleanup refused because daemon is alive; use session stop first; liveness status={}, state_dir={}",
            liveness.status.as_str(),
            state_dir.display()
        )));
    }

    let mut files = Vec::new();
    let mut removed_count = 0usize;
    for path in session_stale_cleanup_candidates(&state_dir)? {
        let existed = path.exists();
        let removed = existed && !dry_run;
        if removed {
            let metadata = fs::metadata(&path).map_err(|err| {
                CliError::runtime_not_running(format!(
                    "failed to inspect stale session file {}: {err}",
                    path.display()
                ))
            })?;
            if !metadata.is_file() {
                return Err(CliError::runtime_not_running(format!(
                    "refusing to remove non-file stale session path {}",
                    path.display()
                )));
            }
            fs::remove_file(&path).map_err(|err| {
                CliError::runtime_not_running(format!(
                    "failed to remove stale session file {}: {err}",
                    path.display()
                ))
            })?;
            removed_count += 1;
        }
        files.push(json!({
            "path": path.display().to_string(),
            "existed": existed,
            "removed": removed
        }));
    }

    Ok(json!({
        "status": if dry_run { "planned" } else { "cleaned" },
        "mode": "stale_session_cleanup",
        "dry_run": dry_run,
        "state_dir": state_dir.display().to_string(),
        "liveness": liveness_json,
        "removed_count": removed_count,
        "files": files,
        "journal_preserved": true
    }))
}

fn session_stale_cleanup_candidates(state_dir: &Path) -> CliOutcome<Vec<PathBuf>> {
    let mut paths = vec![
        session_info_path(state_dir),
        session_heartbeat_path(state_dir),
        session_stop_path(state_dir),
    ];
    for dir in [
        session_requests_dir(state_dir),
        session_responses_dir(state_dir),
    ] {
        if !dir.exists() {
            continue;
        }
        let entries = fs::read_dir(&dir).map_err(|err| {
            CliError::runtime_not_running(format!(
                "failed to read stale session queue directory {}: {err}",
                dir.display()
            ))
        })?;
        for entry in entries {
            let path = entry
                .map_err(|err| {
                    CliError::runtime_not_running(format!(
                        "failed to read stale session queue entry {}: {err}",
                        dir.display()
                    ))
                })?
                .path();
            if path.extension().and_then(|value| value.to_str()) == Some("json") {
                paths.push(path);
            }
        }
    }
    paths.sort();
    Ok(paths)
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
        let response = execute_session_command_request(request.clone(), state_dir);
        let response_path =
            session_responses_dir(state_dir).join(format!("{}.json", response.request_id));
        write_json_file_atomic(&response_path, &response)?;
        fs::remove_file(&path).map_err(|err| {
            CliError::runtime_not_running(format!(
                "failed to remove processed request {}: {err}",
                path.display()
            ))
        })?;
        append_session_request_journal(state_dir, &request, &response)?;
        processed += 1;
    }
    Ok(processed)
}

fn append_session_request_journal(
    state_dir: &Path,
    request: &SessionCommandRequest,
    response: &SessionCommandResponse,
) -> CliOutcome<()> {
    let entry = SessionRequestJournalEntry {
        request_id: request.request_id.clone(),
        command: request.command.clone(),
        args: request.args.clone(),
        lease: request.lease.clone(),
        ok: response.ok,
        error: response.error.clone(),
        created_at_unix_ms: request.created_at_unix_ms,
        started_at_unix_ms: response.started_at_unix_ms,
        completed_at_unix_ms: response.completed_at_unix_ms,
    };
    let journal_path = session_request_journal_path(state_dir);
    rotate_session_request_journal_if_needed(state_dir, &journal_path)?;
    write_json_line(&journal_path, &entry)
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
        "status" => {
            let flags = FlagArgs::parse(&request.args)?;
            flags.expect_positionals("session request status", 0)?;
            let diagnostics = flags.bool("--diagnostics");
            let config = if diagnostics {
                Some(read_user_config()?)
            } else {
                None
            };
            session_status_payload_with_config(state_dir, diagnostics, config.as_ref())
        }
        "journal" => {
            let flags = FlagArgs::parse(&request.args)?;
            flags.expect_positionals("session request journal", 0)?;
            let limit = parse_optional_usize(&flags, "--limit", 20)?;
            session_journal_payload(state_dir, limit)
        }
        "events" => {
            let flags = FlagArgs::parse(&request.args)?;
            flags.expect_positionals("session request events", 0)?;
            let limit = parse_optional_usize(&flags, "--limit", 20)?;
            let after_unix_ms = parse_optional_u64(&flags, "--after-unix-ms")?;
            let after_request_id = parse_optional_string_value(&flags, "--after-request-id")?;
            session_events_payload(state_dir, limit, after_unix_ms, after_request_id.as_deref())
        }
        "contract" => {
            let flags = FlagArgs::parse(&request.args)?;
            flags.expect_positionals("session request contract", 0)?;
            Ok(session_access_contract())
        }
        "api" => {
            let flags = FlagArgs::parse(&request.args)?;
            flags.expect_positionals("session request api", 0)?;
            Ok(session_api_contract())
        }
        "transport" => {
            let flags = FlagArgs::parse(&request.args)?;
            flags.expect_positionals("session request transport", 0)?;
            Ok(session_transport_contract())
        }
        "capabilities" => {
            let flags = FlagArgs::parse(&request.args)?;
            flags.expect_positionals("session request capabilities", 0)?;
            let global = request.global.to_global()?;
            run_capabilities(&global)
        }
        "devices" => {
            let global = request.global.to_global()?;
            run_devices(&global, &request.args)
        }
        "lease" => {
            let global = request.global.to_global()?;
            run_session_lease_in_state_dir(&global, &request.args, state_dir)
        }
        "record" => {
            let global = request.global.to_global()?;
            run_session_record_in_state_dir(&global, &request.args, state_dir)
        }
        "capture_diagnose" => {
            let global = request.global.to_global()?;
            let flags = FlagArgs::parse(&request.args)?;
            run_capture_diagnose(&global, &flags)
        }
        "capture" => {
            let global = request.global.to_global()?;
            run_capture(&global, &request.args)
        }
        "stream" => {
            let flags = FlagArgs::parse(&request.args)?;
            if stream_input_relay_requested(&flags) {
                ensure_session_request_lease(state_dir, request)?;
            }
            let global = request.global.to_global()?;
            run_stream(&global, &request.args)
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
        "instance" => {
            if matches!(
                request.args.first().map(String::as_str),
                Some("app" | "connect" | "reconnect")
            ) {
                ensure_session_request_lease(state_dir, request)?;
            }
            let global = request.global.to_global()?;
            run_session_instance(&global, &request.args)
        }
        "app" => {
            ensure_session_request_lease(state_dir, request)?;
            let global = request.global.to_global()?;
            run_session_app(&global, &request.args)
        }
        "lab_run" => {
            ensure_session_request_lease(state_dir, request)?;
            let global = request.global.to_global()?;
            run_lab("run", &global, &request.args)
        }
        "package_run" => {
            ensure_session_request_lease(state_dir, request)?;
            let global = request.global.to_global()?;
            run_package("run", &global, &request.args)
        }
        "operation_run" => {
            ensure_session_request_lease(state_dir, request)?;
            let global = request.global.to_global()?;
            run_operation("run", &global, &request.args)
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
            let flags = FlagArgs::parse(&request.args)?;
            if !flags.bool("--stale-capture") {
                ensure_session_request_lease(state_dir, request)?;
            }
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
    let action = args.first().map(String::as_str).ok_or_else(|| {
        CliError::usage(
            "session instance requires list|registry|connect|health|keep-alive|reconnect|app",
        )
    })?;
    if action == "app" {
        if args.get(1).is_none() {
            return Err(CliError::usage(
                "session instance app requires launch|stop|restart",
            ));
        }
        return run_session_app(global, &args[1..]);
    }
    let flags = FlagArgs::parse(&args[1..])?;
    let should_route = if matches!(action, "connect" | "reconnect") {
        should_route_control_via_session_daemon(global, &flags)?
    } else {
        should_route_readonly_via_session_daemon(global, &flags)?
    };
    if should_route {
        return submit_session_instance_request(global, &flags, args);
    }
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
        "connect" | "health" | "keep-alive" | "reconnect" => {
            let instance_id = resolve_instance_id_for_flags(global, &config, &flags)?;
            let device_config = device_config_for_instance(global, &config, Some(&instance_id))?;
            let serial = device_config.target.resolved_serial();
            let adb = Adb::new(device_config.adb.clone());
            let state = adb
                .ensure_device(&serial, device_config.target.connect)
                .map_err(|err| CliError::device(err.to_string()))?;
            let screen_size = adb
                .screen_size(&serial)
                .map_err(|err| CliError::device(err.to_string()))?;
            let requested = device_config.capture_backend;
            let fresh_delay = parse_optional_duration_ms(&flags, "--fresh-delay-ms", 160)?;
            let capture_report = if action == "health" && flags.bool("--capture-diagnose") {
                Some(capture_fresh_probe_report(
                    &device_config,
                    requested,
                    fresh_delay,
                )?)
            } else {
                None
            };
            let capture_status = capture_report.as_ref().map(|report| report.status);
            let capture = capture_report
                .as_ref()
                .map(|report| capture_fresh_probe_report_json(report, requested));
            Ok(json!({
                "instance": instance_id,
                "serial": serial,
                "status": instance_health_status(capture_status),
                "state": state,
                "screen_size": screen_size,
                "action": action,
                "keep_alive": action == "keep-alive",
                "capture": capture
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
    if should_route_control_via_session_daemon(global, &flags)? {
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
    run_session_lease_inner(global, args, None)
}

fn run_session_lease_in_state_dir(
    global: &GlobalOptions,
    args: &[String],
    state_dir: &Path,
) -> CliOutcome<Value> {
    run_session_lease_inner(global, args, Some(state_dir))
}

fn run_session_lease_inner(
    global: &GlobalOptions,
    args: &[String],
    forced_state_dir: Option<&Path>,
) -> CliOutcome<Value> {
    let action = args.first().map(String::as_str).ok_or_else(|| {
        CliError::usage("session lease requires acquire|release|preempt|status|run")
    })?;
    if action == "run" {
        return run_session_lease_run(global, &args[1..], forced_state_dir);
    }
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
    let holder = flags
        .optional("--holder")
        .or_else(|| flags.optional("--lease-holder"))
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
        "release" => release_session_lease_file(
            &lease_path,
            &instance_id,
            &holder,
            flags.optional("--lease-id"),
            flags.bool("--force"),
        ),
        other => Err(CliError::usage(format!(
            "unknown session lease action: {other}"
        ))),
    }
}

fn run_session_lease_run(
    global: &GlobalOptions,
    args: &[String],
    forced_state_dir: Option<&Path>,
) -> CliOutcome<Value> {
    if forced_state_dir.is_some() {
        return Err(CliError::usage(
            "session lease run is a local CLI wrapper; do not call it through session request lease",
        ));
    }
    let (lease_args, command_args) = split_lease_run_args(args)?;
    let flags = FlagArgs::parse(&lease_args)?;
    validate_lease_run_flags(&flags)?;
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
        .or_else(|| flags.optional("--lease-holder"))
        .filter(|value| value != "true")
        .unwrap_or_else(|| "manual".to_string());
    let lease_path = session_lease_path(&state_dir, &instance_id);
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

    let mut request_args = command_args;
    request_args.push("--state-dir".to_string());
    request_args.push(state_dir.display().to_string());
    if let Some(timeout) = flags.optional("--request-timeout-ms") {
        request_args.push("--request-timeout-ms".to_string());
        request_args.push(timeout);
    }
    request_args.push("--lease-holder".to_string());
    request_args.push(lease.holder.clone());
    request_args.push("--lease-id".to_string());
    request_args.push(lease.lease_id.clone());

    let command_result = run_session_request(global, &request_args);
    let release_result = release_session_lease_file(
        &lease_path,
        &lease.instance,
        &lease.holder,
        Some(lease.lease_id.clone()),
        false,
    );

    match (command_result, release_result) {
        (Ok(command), Ok(release)) => Ok(json!({
            "status": "completed",
            "mode": "lease_run",
            "lease": lease,
            "command": command,
            "release": release
        })),
        (Err(command), Ok(_)) => Err(command),
        (Ok(_), Err(release)) => Err(release),
        (Err(command), Err(release)) => Err(CliError::runtime_not_running(format!(
            "session lease run command failed with {}: {}; additionally lease release failed with {}: {}",
            command.code, command.message, release.code, release.message
        ))),
    }
}

fn split_lease_run_args(args: &[String]) -> CliOutcome<(Vec<String>, Vec<String>)> {
    let Some(index) = args.iter().position(|arg| arg == "--") else {
        return Err(CliError::usage(
            "session lease run requires '--' before the daemon command, for example: session lease run --holder manual -- tap 100 200",
        ));
    };
    let command_args = args[index + 1..].to_vec();
    if command_args.is_empty() {
        return Err(CliError::usage(
            "session lease run requires a daemon command after '--'",
        ));
    }
    Ok((args[..index].to_vec(), command_args))
}

fn validate_lease_run_flags(flags: &FlagArgs) -> CliOutcome<()> {
    let allowed = [
        "--state-dir",
        "--request-timeout-ms",
        "--holder",
        "--lease-holder",
        "--lease-id",
    ];
    let unexpected = flags
        .flags
        .keys()
        .filter(|flag| !allowed.contains(&flag.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    if unexpected.is_empty() {
        Ok(())
    } else {
        Err(CliError::usage(format!(
            "session lease run lease options do not accept: {}",
            unexpected.join(", ")
        )))
    }
}

fn release_session_lease_file(
    lease_path: &Path,
    instance_id: &str,
    holder: &str,
    lease_id: Option<String>,
    force: bool,
) -> CliOutcome<Value> {
    let Some(lease) = read_json_file::<SessionLease>(lease_path)? else {
        return Ok(json!({
            "status": "not_held",
            "instance": instance_id,
            "path": lease_path.display().to_string()
        }));
    };
    validate_lease_release(&lease, holder, lease_id, force)?;
    fs::remove_file(lease_path).map_err(|err| {
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
    run_session_record_inner(global, args, None)
}

fn run_session_record_in_state_dir(
    global: &GlobalOptions,
    args: &[String],
    state_dir: &Path,
) -> CliOutcome<Value> {
    run_session_record_inner(global, args, Some(state_dir))
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

    fn values(&self, name: &str) -> Vec<String> {
        self.flags.get(name).cloned().unwrap_or_default()
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
    let mut target = DeviceTarget::default();
    if let Some(serial) = instance.and_then(|instance| instance.serial.clone()) {
        target.serial = Some(serial);
    } else if global.instance.as_deref() == Some(instance_id.as_str()) && instance.is_none() {
        target.serial = Some(instance_id.clone());
    }
    let adb = AdbConfig {
        adb_path: effective_adb_path_for_instance(config, instance)?.path,
        ..Default::default()
    };
    let capture_backend = effective_capture_backend_choice(global, &instance_id, instance)?;
    Ok(DeviceRuntimeConfig {
        adb,
        target,
        capture_backend,
    })
}

#[derive(Debug)]
struct DeviceRuntimeConfig {
    adb: AdbConfig,
    target: DeviceTarget,
    capture_backend: CaptureBackendChoice,
}

impl DeviceRuntimeConfig {
    fn capture_backend_config(&self) -> CaptureBackendConfig {
        CaptureBackendConfig::new(self.adb.clone(), self.target.clone())
            .with_requested(self.capture_backend)
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

fn write_json_line<T>(path: &Path, value: &T) -> CliOutcome<()>
where
    T: Serialize,
{
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            CliError::runtime_not_running(format!(
                "failed to create journal directory {}: {err}",
                parent.display()
            ))
        })?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|err| {
            CliError::runtime_not_running(format!(
                "failed to open journal {}: {err}",
                path.display()
            ))
        })?;
    serde_json::to_writer(&mut file, value)
        .map_err(|err| CliError::runtime_not_running(format!("failed to encode journal: {err}")))?;
    file.write_all(b"\n").map_err(|err| {
        CliError::runtime_not_running(format!("failed to write journal {}: {err}", path.display()))
    })?;
    file.flush().map_err(|err| {
        CliError::runtime_not_running(format!("failed to flush journal {}: {err}", path.display()))
    })
}

fn rotate_session_request_journal_if_needed(state_dir: &Path, path: &Path) -> CliOutcome<()> {
    if !path.exists() {
        return Ok(());
    }
    let bytes = file_size_if_exists(path)?;
    if bytes <= SESSION_REQUEST_JOURNAL_MAX_BYTES {
        return Ok(());
    }
    let archive_path = session_request_journal_archive_path(state_dir);
    if archive_path.exists() {
        fs::remove_file(&archive_path).map_err(|err| {
            CliError::runtime_not_running(format!(
                "failed to remove old journal archive {}: {err}",
                archive_path.display()
            ))
        })?;
    }
    fs::rename(path, &archive_path).map_err(|err| {
        CliError::runtime_not_running(format!(
            "failed to rotate journal {} to {}: {err}",
            path.display(),
            archive_path.display()
        ))
    })
}

fn read_session_request_journal(
    state_dir: &Path,
    limit: usize,
) -> CliOutcome<Vec<SessionRequestJournalEntry>> {
    let path = session_request_journal_path(state_dir);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = fs::read_to_string(&path).map_err(|err| {
        CliError::runtime_not_running(format!("failed to read journal {}: {err}", path.display()))
    })?;
    let lines = text
        .lines()
        .enumerate()
        .filter(|(_line_no, line)| !line.trim().is_empty())
        .collect::<Vec<_>>();
    let skip = lines.len().saturating_sub(limit);
    lines
        .into_iter()
        .skip(skip)
        .map(|(line_no, line)| {
            serde_json::from_str::<SessionRequestJournalEntry>(line).map_err(|err| {
                CliError::runtime_not_running(format!(
                    "failed to parse journal {} line {}: {err}",
                    path.display(),
                    line_no + 1
                ))
            })
        })
        .collect()
}

fn count_session_request_journal_entries(state_dir: &Path) -> CliOutcome<usize> {
    let path = session_request_journal_path(state_dir);
    if !path.exists() {
        return Ok(0);
    }
    let text = fs::read_to_string(&path).map_err(|err| {
        CliError::runtime_not_running(format!("failed to read journal {}: {err}", path.display()))
    })?;
    let mut count = 0usize;
    for (line_no, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        serde_json::from_str::<SessionRequestJournalEntry>(line).map_err(|err| {
            CliError::runtime_not_running(format!(
                "failed to parse journal {} line {}: {err}",
                path.display(),
                line_no + 1
            ))
        })?;
        count += 1;
    }
    Ok(count)
}

fn file_size_if_exists(path: &Path) -> CliOutcome<u64> {
    if !path.exists() {
        return Ok(0);
    }
    let metadata = fs::metadata(path).map_err(|err| {
        CliError::runtime_not_running(format!("failed to stat {}: {err}", path.display()))
    })?;
    Ok(metadata.len())
}

fn count_files_with_extension(dir: &Path, extension: &str) -> CliOutcome<usize> {
    if !dir.exists() {
        return Ok(0);
    }
    let entries = fs::read_dir(dir).map_err(|err| {
        CliError::runtime_not_running(format!(
            "failed to read session directory {}: {err}",
            dir.display()
        ))
    })?;
    let mut count = 0usize;
    for entry in entries {
        let path = entry
            .map_err(|err| {
                CliError::runtime_not_running(format!(
                    "failed to read session directory entry {}: {err}",
                    dir.display()
                ))
            })?
            .path();
        if path.extension().and_then(|value| value.to_str()) == Some(extension) {
            count += 1;
        }
    }
    Ok(count)
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

fn session_request_journal_path(state_dir: &Path) -> PathBuf {
    state_dir.join(SESSION_REQUEST_JOURNAL_FILE)
}

fn session_request_journal_archive_path(state_dir: &Path) -> PathBuf {
    state_dir.join(SESSION_REQUEST_JOURNAL_ARCHIVE_FILE)
}

fn current_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn optional_u64_text(value: Option<u64>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "none".to_string())
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
            "instance config keys use instance.<id>.serial|game|server|package|adb_path|capture_backend",
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
        other => return Err(CliError::usage(format!("unknown instance field: {other}"))),
    };
    Ok(json!(value))
}

fn set_instance_value(config: &mut UserConfig, key: &str, value: &str) -> CliOutcome<()> {
    let parts = key.split('.').collect::<Vec<_>>();
    if parts.len() != 3 {
        return Err(CliError::usage(
            "instance config keys use instance.<id>.serial|game|server|package|adb_path|capture_backend",
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
        other => return Err(CliError::usage(format!("unknown instance field: {other}"))),
    }
    Ok(())
}

fn effective_adb_path(config: &UserConfig) -> CliOutcome<actingcommand_device::ResolvedAdbPath> {
    resolve_adb_path(config.adb_path.as_deref()).map_err(|err| CliError::device(err.to_string()))
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
        let requested = device_config.capture_backend;
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
        command_cap("session cleanup", ["offline"], "available"),
        command_cap("session journal", ["offline"], "available"),
        command_cap("session events", ["offline"], "available"),
        command_cap("session contract", ["offline"], "available"),
        command_cap("session api", ["offline"], "available"),
        command_cap("session transport", ["offline"], "available"),
        command_cap("session request status", ["running_runtime"], "available"),
        command_cap("session request journal", ["running_runtime"], "available"),
        command_cap("session request events", ["running_runtime"], "available"),
        command_cap("session request contract", ["running_runtime"], "available"),
        command_cap("session request api", ["running_runtime"], "available"),
        command_cap(
            "session request transport",
            ["running_runtime"],
            "available",
        ),
        command_cap(
            "session request capabilities",
            ["running_runtime"],
            "available",
        ),
        command_cap("session request devices", ["running_runtime"], "available"),
        command_cap("session request lease", ["running_runtime"], "available"),
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
        command_cap(
            "session request instance health",
            ["running_runtime", "device"],
            "available",
        ),
        command_cap(
            "session request instance keep-alive",
            ["running_runtime", "device"],
            "available",
        ),
        command_cap(
            "session request instance connect",
            ["running_runtime", "device", "lab_lease"],
            "available",
        ),
        command_cap(
            "session request instance reconnect",
            ["running_runtime", "device", "lab_lease"],
            "available",
        ),
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
        command_cap("session instance", ["offline", "device"], "available"),
        command_cap("session instance list", ["offline"], "available"),
        command_cap("session instance registry", ["offline"], "available"),
        command_cap("session instance health", ["device"], "available"),
        command_cap("session instance keep-alive", ["device"], "available"),
        command_cap("session instance connect", ["device"], "available"),
        command_cap("session instance reconnect", ["device"], "available"),
        command_cap("session instance app", ["device"], "available"),
        command_cap("session instance app launch", ["device"], "available"),
        command_cap("session instance app stop", ["device"], "available"),
        command_cap("session instance app restart", ["device"], "available"),
        command_cap("session app", ["device"], "available"),
        command_cap("session app launch", ["device"], "available"),
        command_cap("session app stop", ["device"], "available"),
        command_cap("session app restart", ["device"], "available"),
        command_cap("session capture", ["device"], "available"),
        command_cap("session capture diagnose", ["device"], "available"),
        command_cap("session recover", ["device"], "available"),
        command_cap("session lease", ["offline"], "available"),
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
        command_cap("lab status", ["offline"], "available"),
        command_cap("lab lease", ["offline", "lab_lease"], "available"),
        command_cap("lab lease status", ["offline", "lab_lease"], "available"),
        command_cap("lab preempt", ["offline", "lab_lease"], "available"),
        command_cap("lab release", ["offline", "lab_lease"], "available"),
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
    fn runtime_endpoint_policy_allows_loopback_without_auth() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe {
            env::remove_var(TRUSTED_REMOTE_TOKEN_ENV);
            env::remove_var(TRUSTED_REMOTE_CLIENT_CERT_ENV);
        }
        let policy = runtime_endpoint_policy("http://127.0.0.1:4317").unwrap();
        assert_eq!(policy.channel, RuntimeEndpointChannel::LocalDirect);
        assert_eq!(policy.scheme, "http");
        assert_eq!(policy.host, "127.0.0.1");
        assert_eq!(policy.port, 4317);
        assert_eq!(policy.auth_material, None);
    }

    #[test]
    fn runtime_endpoint_policy_blocks_remote_http() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe {
            env::remove_var(TRUSTED_REMOTE_TOKEN_ENV);
            env::remove_var(TRUSTED_REMOTE_CLIENT_CERT_ENV);
        }
        let err = runtime_endpoint_policy("http://example.invalid:4317").unwrap_err();
        assert_eq!(err.code, "trusted_remote_transport_blocked");
        assert_eq!(err.exit_code(), 3);
    }

    #[test]
    fn runtime_endpoint_policy_blocks_remote_https_without_auth() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe {
            env::remove_var(TRUSTED_REMOTE_TOKEN_ENV);
            env::remove_var(TRUSTED_REMOTE_CLIENT_CERT_ENV);
        }
        let err = runtime_endpoint_policy("https://example.invalid:4317").unwrap_err();
        assert_eq!(err.code, "trusted_remote_auth_required");
        assert_eq!(err.exit_code(), 3);
    }

    #[test]
    fn runtime_endpoint_policy_accepts_remote_https_with_token() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe {
            env::set_var(TRUSTED_REMOTE_TOKEN_ENV, "test-token");
            env::remove_var(TRUSTED_REMOTE_CLIENT_CERT_ENV);
        }
        let policy = runtime_endpoint_policy("https://example.invalid:4317").unwrap();
        unsafe {
            env::remove_var(TRUSTED_REMOTE_TOKEN_ENV);
        }
        assert_eq!(policy.channel, RuntimeEndpointChannel::TrustedRemote);
        assert_eq!(policy.scheme, "https");
        assert_eq!(policy.auth_material, Some("token"));
    }

    #[test]
    fn status_blocks_untrusted_remote_runtime_endpoint() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe {
            env::remove_var(CONFIG_ENV);
            env::remove_var(TRUSTED_REMOTE_TOKEN_ENV);
            env::remove_var(TRUSTED_REMOTE_CLIENT_CERT_ENV);
        }
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
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe {
            env::remove_var(CONFIG_ENV);
            env::remove_var(TRUSTED_REMOTE_TOKEN_ENV);
            env::remove_var(TRUSTED_REMOTE_CLIENT_CERT_ENV);
        }
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
    fn scheduler_stub_is_exit_six() {
        let result = run_cli(["--json", "scheduler", "status"], true);
        assert_eq!(result.exit_code(), 6);
        assert_eq!(
            result.envelope.error.as_ref().unwrap().code,
            "scheduler_not_available"
        );
    }

    #[test]
    fn lab_status_alias_uses_session_status_without_runtime_endpoint() {
        let temp = TempDir::new().unwrap();
        let result = run_cli(
            [
                "--json",
                "lab",
                "status",
                "--state-dir",
                temp.path().to_str().unwrap(),
                "--diagnostics",
            ],
            true,
        );

        assert_eq!(result.exit_code(), 0);
        let data = result.envelope.data.as_ref().unwrap();
        assert_eq!(data.get("running").and_then(Value::as_bool), Some(false));
        assert_eq!(
            data.pointer("/diagnostics/leases/active_count")
                .and_then(Value::as_u64),
            Some(0)
        );
    }

    #[test]
    fn lab_lease_and_release_alias_session_lease_files() {
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        unsafe {
            env::set_var(CONFIG_ENV, &config);
        }
        let leased = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "lab",
                "lease",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--holder",
                "lab",
                "--lease-id",
                "lease-1",
            ],
            true,
        );
        let released = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "lab",
                "release",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--holder",
                "lab",
                "--lease-id",
                "lease-1",
            ],
            true,
        );
        unsafe {
            env::remove_var(CONFIG_ENV);
        }

        assert_eq!(leased.exit_code(), 0);
        assert_eq!(
            leased
                .envelope
                .data
                .as_ref()
                .unwrap()
                .pointer("/lease/holder")
                .and_then(Value::as_str),
            Some("lab")
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
        assert!(!session_lease_path(&state_dir, "ak").exists());
    }

    #[test]
    fn lab_lease_status_alias_reads_session_lease_file() {
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        unsafe {
            env::set_var(CONFIG_ENV, &config);
        }
        let leased = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "lab",
                "lease",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--holder",
                "scheduler",
                "--lease-id",
                "scheduler-lease",
            ],
            true,
        );
        let status = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "lab",
                "lease",
                "status",
                "--state-dir",
                state_dir.to_str().unwrap(),
            ],
            true,
        );
        unsafe {
            env::remove_var(CONFIG_ENV);
        }

        assert_eq!(leased.exit_code(), 0);
        assert_eq!(status.exit_code(), 0);
        let data = status.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.pointer("/lease/holder").and_then(Value::as_str),
            Some("scheduler")
        );
        assert_eq!(
            data.pointer("/lease/lease_id").and_then(Value::as_str),
            Some("scheduler-lease")
        );
    }

    #[test]
    fn lab_preempt_alias_records_previous_session_lease() {
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        unsafe {
            env::set_var(CONFIG_ENV, &config);
        }
        let scheduler_lease = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "lab",
                "lease",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--holder",
                "scheduler",
                "--lease-id",
                "scheduler-lease",
            ],
            true,
        );
        let lab_preempt = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "lab",
                "preempt",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--holder",
                "lab",
                "--lease-id",
                "lab-lease",
            ],
            true,
        );
        unsafe {
            env::remove_var(CONFIG_ENV);
        }

        assert_eq!(scheduler_lease.exit_code(), 0);
        assert_eq!(lab_preempt.exit_code(), 0);
        let data = lab_preempt.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.get("status").and_then(Value::as_str),
            Some("preempted")
        );
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
    fn config_set_and_get_instance_adb_and_capture_backend() {
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        unsafe {
            env::set_var(CONFIG_ENV, &config);
        }
        let adb = run_cli(
            [
                "--json",
                "config",
                "set",
                "instance.ak-b.adb_path",
                "C:\\Tools\\adb.exe",
            ],
            true,
        );
        let backend = run_cli(
            [
                "--json",
                "config",
                "set",
                "instance.ak-b.capture_backend",
                "nemu_ipc",
            ],
            true,
        );
        let get_adb = run_cli(["--json", "config", "get", "instance.ak-b.adb_path"], true);
        let get_backend = run_cli(
            ["--json", "config", "get", "instance.ak-b.capture_backend"],
            true,
        );
        unsafe {
            env::remove_var(CONFIG_ENV);
        }

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
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        unsafe {
            env::set_var(CONFIG_ENV, &config);
        }
        let result = run_cli(
            [
                "--json",
                "config",
                "set",
                "instance.ak-b.capture_backend",
                "not-a-backend",
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

    fn write_test_session_files(state_dir: &Path) {
        let info = SessionInfo {
            pid: 321,
            started_at_unix_ms: 10,
            state_dir: state_dir.display().to_string(),
            runtime_version: RUNTIME_VERSION.to_string(),
        };
        let heartbeat = SessionHeartbeat {
            pid: 321,
            updated_at_unix_ms: current_unix_ms(),
            state: "idle".to_string(),
        };
        write_json_file_atomic(&session_info_path(state_dir), &info).unwrap();
        write_json_file_atomic(&session_heartbeat_path(state_dir), &heartbeat).unwrap();
    }

    fn write_test_session_info_only(state_dir: &Path) {
        let info = SessionInfo {
            pid: 321,
            started_at_unix_ms: 10,
            state_dir: state_dir.display().to_string(),
            runtime_version: RUNTIME_VERSION.to_string(),
        };
        write_json_file_atomic(&session_info_path(state_dir), &info).unwrap();
    }

    fn write_test_session_heartbeat(state_dir: &Path, pid: u32, updated_at_unix_ms: u64) {
        let heartbeat = SessionHeartbeat {
            pid,
            updated_at_unix_ms,
            state: "idle".to_string(),
        };
        write_json_file_atomic(&session_heartbeat_path(state_dir), &heartbeat).unwrap();
    }

    fn assert_daemon_request_timeout(result: CliResult) {
        assert_eq!(result.exit_code(), 5);
        assert_eq!(
            result.envelope.error.as_ref().unwrap().code,
            "runtime_not_running"
        );
        assert!(
            result
                .envelope
                .error
                .as_ref()
                .unwrap()
                .message
                .contains("timed out")
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
    fn session_liveness_diagnostics_classifies_heartbeat_state() {
        let info = SessionInfo {
            pid: 321,
            started_at_unix_ms: 10,
            state_dir: "state".to_string(),
            runtime_version: RUNTIME_VERSION.to_string(),
        };
        let heartbeat = SessionHeartbeat {
            pid: 321,
            updated_at_unix_ms: 1_000,
            state: "idle".to_string(),
        };

        let alive = session_liveness_diagnostics(Some(&info), Some(&heartbeat), 2_000);
        assert_eq!(alive.get("status").and_then(Value::as_str), Some("alive"));
        assert_eq!(
            alive.get("heartbeat_age_ms").and_then(Value::as_u64),
            Some(1_000)
        );
        assert_eq!(
            alive.get("can_accept_requests").and_then(Value::as_bool),
            Some(true)
        );

        let stale = session_liveness_diagnostics(Some(&info), Some(&heartbeat), 4_001);
        assert_eq!(stale.get("status").and_then(Value::as_str), Some("stale"));
        assert_eq!(
            stale.get("can_accept_requests").and_then(Value::as_bool),
            Some(false)
        );

        let missing_heartbeat = session_liveness_diagnostics(Some(&info), None, 2_000);
        assert_eq!(
            missing_heartbeat.get("status").and_then(Value::as_str),
            Some("heartbeat_missing")
        );

        let mismatched = SessionHeartbeat {
            pid: 999,
            updated_at_unix_ms: 1_900,
            state: "idle".to_string(),
        };
        let pid_mismatch = session_liveness_diagnostics(Some(&info), Some(&mismatched), 2_000);
        assert_eq!(
            pid_mismatch.get("status").and_then(Value::as_str),
            Some("pid_mismatch")
        );
    }

    #[test]
    fn session_daemon_info_exists_requires_alive_heartbeat() {
        let temp = TempDir::new().unwrap();
        let flags = FlagArgs::parse(&[
            "--state-dir".to_string(),
            temp.path().to_str().unwrap().to_string(),
        ])
        .unwrap();

        assert!(!session_daemon_info_exists(&flags).unwrap());

        write_test_session_info_only(temp.path());
        assert!(!session_daemon_info_exists(&flags).unwrap());

        write_test_session_heartbeat(
            temp.path(),
            321,
            current_unix_ms().saturating_sub(SESSION_HEARTBEAT_STALE_MS + 1),
        );
        assert!(!session_daemon_info_exists(&flags).unwrap());

        write_test_session_heartbeat(temp.path(), 999, current_unix_ms());
        assert!(!session_daemon_info_exists(&flags).unwrap());

        write_test_session_heartbeat(temp.path(), 321, current_unix_ms());
        assert!(session_daemon_info_exists(&flags).unwrap());
    }

    #[test]
    fn require_session_blocks_device_command_without_alive_daemon() {
        let temp = TempDir::new().unwrap();
        let out = temp.path().join("frame.png");

        let result = run_cli(
            [
                "--json",
                "--require-session",
                "capture",
                "--out",
                out.to_str().unwrap(),
                "--state-dir",
                temp.path().to_str().unwrap(),
            ],
            true,
        );

        assert_eq!(result.exit_code(), 3);
        let error = result.envelope.error.as_ref().unwrap();
        assert_eq!(error.code, "session_daemon_required");
        assert_eq!(
            error.blocked_by,
            vec!["session_layer".to_string(), "running_runtime".to_string()]
        );
        assert!(!out.exists());
    }

    #[test]
    fn require_session_blocks_explicit_local_bypass() {
        let temp = TempDir::new().unwrap();
        write_test_session_files(temp.path());
        let out = temp.path().join("frame.png");

        let result = run_cli(
            [
                "--json",
                "--require-session",
                "capture",
                "--local",
                "--out",
                out.to_str().unwrap(),
                "--state-dir",
                temp.path().to_str().unwrap(),
            ],
            true,
        );

        assert_eq!(result.exit_code(), 3);
        let error = result.envelope.error.as_ref().unwrap();
        assert_eq!(error.code, "session_daemon_required");
    }

    #[test]
    fn require_session_allows_explicit_daemon_route_to_report_liveness() {
        let temp = TempDir::new().unwrap();
        let out = temp.path().join("frame.png");

        let result = run_cli(
            [
                "--json",
                "--require-session",
                "capture",
                "--via-daemon",
                "--out",
                out.to_str().unwrap(),
                "--state-dir",
                temp.path().to_str().unwrap(),
                "--request-timeout-ms",
                "1",
            ],
            true,
        );

        assert_eq!(result.exit_code(), 5);
        let error = result.envelope.error.as_ref().unwrap();
        assert_eq!(error.code, "runtime_not_running");
    }

    #[test]
    fn require_session_env_flag_enables_session_throat() {
        let global = GlobalOptions {
            json: true,
            require_session: false,
            ..Default::default()
        };
        assert!(session_throat_required_from_env(&global, true));
        assert!(!session_throat_required_from_env(&global, false));

        let explicit = GlobalOptions {
            json: true,
            require_session: true,
            ..Default::default()
        };
        assert!(session_throat_required_from_env(&explicit, false));
    }

    #[test]
    fn strict_session_throat_covers_instance_keep_alive() {
        assert!(session_subcommand_requires_throat(
            "instance",
            &["connect".to_string()]
        ));
        assert!(session_subcommand_requires_throat(
            "instance",
            &["app".to_string(), "launch".to_string()]
        ));
        assert!(session_subcommand_requires_throat(
            "instance",
            &["health".to_string()]
        ));
        assert!(session_subcommand_requires_throat(
            "instance",
            &["keep-alive".to_string()]
        ));
        assert!(session_subcommand_requires_throat(
            "instance",
            &["reconnect".to_string()]
        ));
    }

    #[test]
    fn session_status_via_daemon_with_stale_heartbeat_fails_before_request_write() {
        let temp = TempDir::new().unwrap();
        write_test_session_info_only(temp.path());
        write_test_session_heartbeat(
            temp.path(),
            321,
            current_unix_ms().saturating_sub(SESSION_HEARTBEAT_STALE_MS + 1),
        );

        let result = run_cli(
            [
                "--json",
                "session",
                "status",
                "--via-daemon",
                "--diagnostics",
                "--state-dir",
                temp.path().to_str().unwrap(),
                "--request-timeout-ms",
                "100",
            ],
            true,
        );

        assert_eq!(result.exit_code(), 5);
        let error = result.envelope.error.as_ref().unwrap();
        assert_eq!(error.code, "runtime_not_running");
        assert!(error.message.contains("liveness status=stale"));
        assert!(!session_requests_dir(temp.path()).exists());
    }

    #[test]
    fn session_start_existing_alive_state_is_already_running() {
        let temp = TempDir::new().unwrap();
        write_test_session_files(temp.path());

        let result = run_cli(
            [
                "--json",
                "session",
                "start",
                "--state-dir",
                temp.path().to_str().unwrap(),
            ],
            true,
        );

        assert_eq!(result.exit_code(), 0);
        let data = result.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.get("status").and_then(Value::as_str),
            Some("already_running")
        );
        assert_eq!(
            data.pointer("/liveness/status").and_then(Value::as_str),
            Some("alive")
        );
    }

    #[test]
    fn session_start_existing_missing_heartbeat_fails_visibly() {
        let temp = TempDir::new().unwrap();
        write_test_session_info_only(temp.path());

        let result = run_cli(
            [
                "--json",
                "session",
                "start",
                "--state-dir",
                temp.path().to_str().unwrap(),
            ],
            true,
        );

        assert_eq!(result.exit_code(), 5);
        let error = result.envelope.error.as_ref().unwrap();
        assert_eq!(error.code, "runtime_not_running");
        assert!(error.message.contains("not accepting requests"));
        assert!(error.message.contains("liveness status=heartbeat_missing"));
    }

    #[test]
    fn session_start_existing_stale_heartbeat_fails_visibly() {
        let temp = TempDir::new().unwrap();
        write_test_session_info_only(temp.path());
        write_test_session_heartbeat(
            temp.path(),
            321,
            current_unix_ms().saturating_sub(SESSION_HEARTBEAT_STALE_MS + 1),
        );

        let result = run_cli(
            [
                "--json",
                "session",
                "start",
                "--state-dir",
                temp.path().to_str().unwrap(),
            ],
            true,
        );

        assert_eq!(result.exit_code(), 5);
        let error = result.envelope.error.as_ref().unwrap();
        assert_eq!(error.code, "runtime_not_running");
        assert!(error.message.contains("not accepting requests"));
        assert!(error.message.contains("liveness status=stale"));
    }

    #[test]
    fn session_stop_existing_missing_heartbeat_fails_before_stop_request() {
        let temp = TempDir::new().unwrap();
        write_test_session_info_only(temp.path());

        let result = run_cli(
            [
                "--json",
                "session",
                "stop",
                "--state-dir",
                temp.path().to_str().unwrap(),
            ],
            true,
        );

        assert_eq!(result.exit_code(), 5);
        let error = result.envelope.error.as_ref().unwrap();
        assert_eq!(error.code, "runtime_not_running");
        assert!(error.message.contains("stop refused"));
        assert!(error.message.contains("liveness status=heartbeat_missing"));
        assert!(!session_stop_path(temp.path()).exists());
    }

    #[test]
    fn session_stop_existing_stale_heartbeat_fails_before_stop_request() {
        let temp = TempDir::new().unwrap();
        write_test_session_info_only(temp.path());
        write_test_session_heartbeat(
            temp.path(),
            321,
            current_unix_ms().saturating_sub(SESSION_HEARTBEAT_STALE_MS + 1),
        );

        let result = run_cli(
            [
                "--json",
                "session",
                "stop",
                "--state-dir",
                temp.path().to_str().unwrap(),
            ],
            true,
        );

        assert_eq!(result.exit_code(), 5);
        let error = result.envelope.error.as_ref().unwrap();
        assert_eq!(error.code, "runtime_not_running");
        assert!(error.message.contains("stop refused"));
        assert!(error.message.contains("liveness status=stale"));
        assert!(!session_stop_path(temp.path()).exists());
    }

    #[test]
    fn session_stop_existing_alive_state_writes_stop_request() {
        let temp = TempDir::new().unwrap();
        write_test_session_files(temp.path());
        let info_path = session_info_path(temp.path());
        let remover = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            let _ = fs::remove_file(info_path);
        });

        let result = run_cli(
            [
                "--json",
                "session",
                "stop",
                "--state-dir",
                temp.path().to_str().unwrap(),
            ],
            true,
        );
        remover.join().unwrap();

        assert_eq!(result.exit_code(), 0);
        let data = result.envelope.data.as_ref().unwrap();
        assert_eq!(data.get("status").and_then(Value::as_str), Some("stopped"));
        assert!(session_stop_path(temp.path()).exists());
    }

    #[test]
    fn session_cleanup_requires_stale_flag() {
        let temp = TempDir::new().unwrap();

        let result = run_cli(
            [
                "--json",
                "session",
                "cleanup",
                "--state-dir",
                temp.path().to_str().unwrap(),
            ],
            true,
        );

        assert_eq!(result.exit_code(), 2);
        let error = result.envelope.error.as_ref().unwrap();
        assert_eq!(error.code, "validation_failed");
        assert!(error.message.contains("requires --stale"));
    }

    #[test]
    fn session_cleanup_refuses_alive_daemon_and_preserves_files() {
        let temp = TempDir::new().unwrap();
        write_test_session_files(temp.path());

        let result = run_cli(
            [
                "--json",
                "session",
                "cleanup",
                "--stale",
                "--state-dir",
                temp.path().to_str().unwrap(),
            ],
            true,
        );

        assert_eq!(result.exit_code(), 5);
        let error = result.envelope.error.as_ref().unwrap();
        assert_eq!(error.code, "runtime_not_running");
        assert!(error.message.contains("cleanup refused"));
        assert!(session_info_path(temp.path()).exists());
        assert!(session_heartbeat_path(temp.path()).exists());
    }

    #[test]
    fn session_cleanup_stale_state_removes_files_and_preserves_journal() {
        let temp = TempDir::new().unwrap();
        write_test_session_info_only(temp.path());
        write_test_session_heartbeat(
            temp.path(),
            321,
            current_unix_ms().saturating_sub(SESSION_HEARTBEAT_STALE_MS + 1),
        );
        fs::write(session_stop_path(temp.path()), "stop").unwrap();
        fs::create_dir_all(session_requests_dir(temp.path())).unwrap();
        fs::create_dir_all(session_responses_dir(temp.path())).unwrap();
        let request_path = session_requests_dir(temp.path()).join("stale-request.json");
        let response_path = session_responses_dir(temp.path()).join("stale-response.json");
        let ignored_path = session_requests_dir(temp.path()).join("note.txt");
        fs::write(&request_path, "{}").unwrap();
        fs::write(&response_path, "{}").unwrap();
        fs::write(&ignored_path, "keep").unwrap();
        fs::write(session_request_journal_path(temp.path()), "{\"ok\":true}\n").unwrap();

        let result = run_cli(
            [
                "--json",
                "session",
                "cleanup",
                "--stale",
                "--state-dir",
                temp.path().to_str().unwrap(),
            ],
            true,
        );

        assert_eq!(result.exit_code(), 0);
        let data = result.envelope.data.as_ref().unwrap();
        assert_eq!(data.get("status").and_then(Value::as_str), Some("cleaned"));
        assert_eq!(data.get("removed_count").and_then(Value::as_u64), Some(5));
        assert!(!session_info_path(temp.path()).exists());
        assert!(!session_heartbeat_path(temp.path()).exists());
        assert!(!session_stop_path(temp.path()).exists());
        assert!(!request_path.exists());
        assert!(!response_path.exists());
        assert!(ignored_path.exists());
        assert!(session_request_journal_path(temp.path()).exists());
    }

    #[test]
    fn session_cleanup_stale_state_dry_run_does_not_remove_files() {
        let temp = TempDir::new().unwrap();
        write_test_session_info_only(temp.path());
        write_test_session_heartbeat(
            temp.path(),
            321,
            current_unix_ms().saturating_sub(SESSION_HEARTBEAT_STALE_MS + 1),
        );

        let result = run_cli(
            [
                "--json",
                "session",
                "cleanup",
                "--stale",
                "--dry-run",
                "--state-dir",
                temp.path().to_str().unwrap(),
            ],
            true,
        );

        assert_eq!(result.exit_code(), 0);
        let data = result.envelope.data.as_ref().unwrap();
        assert_eq!(data.get("status").and_then(Value::as_str), Some("planned"));
        assert_eq!(data.get("removed_count").and_then(Value::as_u64), Some(0));
        assert!(session_info_path(temp.path()).exists());
        assert!(session_heartbeat_path(temp.path()).exists());
    }

    #[test]
    fn status_prefers_daemon_when_session_info_exists() {
        let temp = TempDir::new().unwrap();
        write_test_session_files(temp.path());
        let result = run_cli(
            [
                "--json",
                "status",
                "--state-dir",
                temp.path().to_str().unwrap(),
                "--request-timeout-ms",
                "1",
            ],
            true,
        );

        assert_eq!(result.exit_code(), 5);
        assert_eq!(
            result.envelope.error.as_ref().unwrap().code,
            "runtime_not_running"
        );
        assert!(
            result
                .envelope
                .error
                .as_ref()
                .unwrap()
                .message
                .contains("timed out")
        );
    }

    #[test]
    fn devices_prefers_daemon_when_session_info_exists() {
        let temp = TempDir::new().unwrap();
        write_test_session_files(temp.path());
        let result = run_cli(
            [
                "--json",
                "devices",
                "--state-dir",
                temp.path().to_str().unwrap(),
                "--request-timeout-ms",
                "1",
            ],
            true,
        );

        assert_eq!(result.exit_code(), 5);
        assert_eq!(
            result.envelope.error.as_ref().unwrap().code,
            "runtime_not_running"
        );
        assert!(
            result
                .envelope
                .error
                .as_ref()
                .unwrap()
                .message
                .contains("timed out")
        );
    }

    #[test]
    fn direct_touch_prefers_daemon_when_session_info_exists() {
        let temp = TempDir::new().unwrap();
        write_test_session_files(temp.path());
        let result = run_cli(
            [
                "--json",
                "tap",
                "--state-dir",
                temp.path().to_str().unwrap(),
                "--request-timeout-ms",
                "1",
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
        assert!(
            result
                .envelope
                .error
                .as_ref()
                .unwrap()
                .message
                .contains("timed out")
        );
    }

    #[test]
    fn daemon_internal_handlers_do_not_requeue_to_daemon() {
        let temp = TempDir::new().unwrap();
        write_test_session_files(temp.path());
        let flags = FlagArgs::parse(&[
            "--state-dir".to_string(),
            temp.path().to_str().unwrap().to_string(),
        ])
        .unwrap();
        let client_global = GlobalOptions::default();
        let daemon_global = GlobalOptions {
            inside_session_daemon: true,
            ..Default::default()
        };

        assert!(should_route_readonly_via_session_daemon(&client_global, &flags).unwrap());
        assert!(should_route_control_via_session_daemon(&client_global, &flags).unwrap());
        assert!(!should_route_readonly_via_session_daemon(&daemon_global, &flags).unwrap());
        assert!(!should_route_control_via_session_daemon(&daemon_global, &flags).unwrap());
    }

    #[test]
    fn device_lifecycle_and_run_entrypoints_prefer_daemon_when_session_info_exists() {
        let temp = TempDir::new().unwrap();
        write_test_session_files(temp.path());
        let state_dir = temp.path().to_str().unwrap();
        let input = temp.path().join("input.zip");
        let out = temp.path().join("out.zip");
        let operation_dir = temp.path().join("operation");

        let cases: Vec<Vec<&str>> = vec![
            vec![
                "--json",
                "--instance",
                "ak",
                "monitor",
                "--once",
                "--state-dir",
                state_dir,
                "--request-timeout-ms",
                "1",
            ],
            vec![
                "--json",
                "--instance",
                "ak",
                "monitor",
                "--recover",
                "--capture",
                "--state-dir",
                state_dir,
                "--request-timeout-ms",
                "1",
                "--lease-holder",
                "scheduler",
                "--lease-id",
                "lease-1",
            ],
            vec![
                "--json",
                "--instance",
                "ak",
                "session",
                "instance",
                "health",
                "--state-dir",
                state_dir,
                "--request-timeout-ms",
                "1",
            ],
            vec![
                "--json",
                "--instance",
                "ak",
                "session",
                "instance",
                "reconnect",
                "--state-dir",
                state_dir,
                "--request-timeout-ms",
                "1",
                "--lease-holder",
                "scheduler",
                "--lease-id",
                "lease-1",
            ],
            vec![
                "--json",
                "--instance",
                "ak",
                "session",
                "app",
                "launch",
                "--state-dir",
                state_dir,
                "--request-timeout-ms",
                "1",
                "--lease-holder",
                "scheduler",
                "--lease-id",
                "lease-1",
                "--package",
                "com.example.game",
            ],
            vec![
                "--json",
                "--instance",
                "ak",
                "lab",
                "run",
                "--state-dir",
                state_dir,
                "--request-timeout-ms",
                "1",
                "--lease-holder",
                "scheduler",
                "--lease-id",
                "lease-1",
                "--zip",
                input.to_str().unwrap(),
                "--out",
                out.to_str().unwrap(),
            ],
            vec![
                "--json",
                "--instance",
                "ak",
                "package",
                "run",
                "--state-dir",
                state_dir,
                "--request-timeout-ms",
                "1",
                "--lease-holder",
                "scheduler",
                "--lease-id",
                "lease-1",
                "--zip",
                input.to_str().unwrap(),
                "--out",
                out.to_str().unwrap(),
            ],
            vec![
                "--json",
                "--instance",
                "ak",
                "operation",
                "run",
                "--state-dir",
                state_dir,
                "--request-timeout-ms",
                "1",
                "--lease-holder",
                "scheduler",
                "--lease-id",
                "lease-1",
                "--operation-dir",
                operation_dir.to_str().unwrap(),
            ],
        ];

        for args in cases {
            assert_daemon_request_timeout(run_cli(args, true));
        }
    }

    #[test]
    fn session_status_local_bypasses_daemon_preference() {
        let temp = TempDir::new().unwrap();
        write_test_session_files(temp.path());
        let result = run_cli(
            [
                "--json",
                "session",
                "status",
                "--local",
                "--state-dir",
                temp.path().to_str().unwrap(),
            ],
            true,
        );

        assert_eq!(result.exit_code(), 0);
        let data = result.envelope.data.as_ref().unwrap();
        assert_eq!(data.get("running").and_then(Value::as_bool), Some(true));
        assert_eq!(data.pointer("/info/pid").and_then(Value::as_u64), Some(321));
    }

    #[test]
    fn session_status_via_daemon_without_daemon_is_runtime_error() {
        let temp = TempDir::new().unwrap();
        let result = run_cli(
            [
                "--json",
                "session",
                "status",
                "--via-daemon",
                "--diagnostics",
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
    fn session_journal_via_daemon_without_daemon_is_runtime_error() {
        let temp = TempDir::new().unwrap();
        let result = run_cli(
            [
                "--json",
                "session",
                "journal",
                "--via-daemon",
                "--limit",
                "3",
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
    fn session_lease_run_requires_command_separator() {
        let result = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "lease",
                "run",
                "--holder",
                "manual",
                "tap",
                "100",
                "200",
            ],
            true,
        );

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
                .contains("requires '--'")
        );
    }

    #[test]
    fn session_lease_run_submits_with_generated_lease_and_releases_on_timeout() {
        let temp = TempDir::new().unwrap();
        write_test_session_files(temp.path());
        let result = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "lease",
                "run",
                "--state-dir",
                temp.path().to_str().unwrap(),
                "--request-timeout-ms",
                "1",
                "--holder",
                "manual",
                "--",
                "tap",
                "100",
                "200",
            ],
            true,
        );

        assert_daemon_request_timeout(result);
        assert!(!session_lease_path(temp.path(), "ak").exists());

        let request_paths = fs::read_dir(session_requests_dir(temp.path()))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        assert_eq!(request_paths.len(), 1);
        let request = read_json_file::<SessionCommandRequest>(&request_paths[0])
            .unwrap()
            .unwrap();
        assert_eq!(request.command, "tap");
        assert_eq!(request.args, vec!["100".to_string(), "200".to_string()]);
        let lease = request.lease.unwrap();
        assert_eq!(lease.holder, "manual");
        assert!(
            lease
                .lease_id
                .as_deref()
                .is_some_and(|value| !value.is_empty())
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
    fn stream_command_reports_bounded_dry_run_contract() {
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        unsafe {
            env::set_var(CONFIG_ENV, &config);
        }
        let stream = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "stream",
                "--dry-run",
                "--max-frames",
                "2",
            ],
            true,
        );
        unsafe {
            env::remove_var(CONFIG_ENV);
        }

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
    fn stream_input_relay_dry_run_reports_planned_action() {
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        unsafe {
            env::set_var(CONFIG_ENV, &config);
        }
        let stream = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "stream",
                "--dry-run",
                "--max-frames",
                "1",
                "--input-relay",
                "tap",
                "10",
                "20",
            ],
            true,
        );
        unsafe {
            env::remove_var(CONFIG_ENV);
        }

        assert_eq!(stream.exit_code(), 0);
        let data = stream.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.pointer("/input_relay/status").and_then(Value::as_str),
            Some("planned")
        );
        assert_eq!(
            data.pointer("/input_relay/action/type")
                .and_then(Value::as_str),
            Some("tap")
        );
        assert_eq!(
            data.pointer("/input_relay/action/x")
                .and_then(Value::as_i64),
            Some(10)
        );
        assert_eq!(
            data.pointer("/contract/input_relay/requested")
                .and_then(Value::as_bool),
            Some(true)
        );
        let event_types = data
            .get("events")
            .and_then(Value::as_array)
            .unwrap()
            .iter()
            .map(|event| event.get("type").and_then(Value::as_str).unwrap())
            .collect::<Vec<_>>();
        assert!(event_types.contains(&"stream.input_relay"));
        let relay_event = data
            .get("events")
            .and_then(Value::as_array)
            .unwrap()
            .iter()
            .find(|event| event.get("type").and_then(Value::as_str) == Some("stream.input_relay"))
            .unwrap();
        assert_eq!(
            relay_event.get("schema_version").and_then(Value::as_str),
            Some("session.stream.event.v0.1")
        );
        assert_eq!(
            relay_event.get("stream_id").and_then(Value::as_str),
            data.get("stream_id").and_then(Value::as_str)
        );
    }

    #[test]
    fn stream_input_relay_dry_run_reports_multiple_events() {
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        unsafe {
            env::set_var(CONFIG_ENV, &config);
        }
        let stream = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "stream",
                "--dry-run",
                "--max-frames",
                "1",
                "--input-event",
                "tap,10,20",
                "--input-event",
                "key,back",
            ],
            true,
        );
        unsafe {
            env::remove_var(CONFIG_ENV);
        }

        assert_eq!(stream.exit_code(), 0);
        let data = stream.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.pointer("/input_relay/action_count")
                .and_then(Value::as_u64),
            Some(2)
        );
        assert_eq!(
            data.pointer("/input_relay/actions/0/type")
                .and_then(Value::as_str),
            Some("tap")
        );
        assert_eq!(
            data.pointer("/input_relay/actions/1/type")
                .and_then(Value::as_str),
            Some("key")
        );
        assert_eq!(
            data.pointer("/input_relay/actions/1/key")
                .and_then(Value::as_str),
            Some("4")
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
        for (command, args) in [
            (
                "app",
                vec![
                    "launch".to_string(),
                    "--package".to_string(),
                    "com.example.game".to_string(),
                ],
            ),
            (
                "instance",
                vec![
                    "app".to_string(),
                    "launch".to_string(),
                    "--package".to_string(),
                    "com.example.game".to_string(),
                ],
            ),
        ] {
            let temp = TempDir::new().unwrap();
            let request = SessionCommandRequest {
                request_id: format!("{command}-request"),
                command: command.to_string(),
                global: SessionCommandGlobal {
                    instance: Some("ak".to_string()),
                    game: None,
                    server: None,
                    resource_root: None,
                    capture_backend: None,
                    dry_run: false,
                },
                args,
                lease: None,
                created_at_unix_ms: 1,
            };

            let err = execute_session_command_request_inner(&request, temp.path()).unwrap_err();
            assert_eq!(err.code, "lab_lease_required");
            assert_eq!(err.exit_code(), 3);
        }
    }

    #[test]
    fn session_lab_run_request_requires_lease_before_zip_or_device_io() {
        let temp = TempDir::new().unwrap();
        let request = SessionCommandRequest {
            request_id: "request-1".to_string(),
            command: "lab_run".to_string(),
            global: SessionCommandGlobal {
                instance: Some("ak".to_string()),
                game: None,
                server: None,
                resource_root: None,
                capture_backend: None,
                dry_run: false,
            },
            args: vec![
                "--zip".to_string(),
                temp.path().join("missing.zip").display().to_string(),
                "--out".to_string(),
                temp.path().join("out.zip").display().to_string(),
            ],
            lease: None,
            created_at_unix_ms: 1,
        };

        let err = execute_session_command_request_inner(&request, temp.path()).unwrap_err();
        assert_eq!(err.code, "lab_lease_required");
        assert_eq!(err.exit_code(), 3);
    }

    #[test]
    fn session_package_run_request_requires_lease_before_zip_or_device_io() {
        let temp = TempDir::new().unwrap();
        let request = SessionCommandRequest {
            request_id: "request-1".to_string(),
            command: "package_run".to_string(),
            global: SessionCommandGlobal {
                instance: Some("ak".to_string()),
                game: None,
                server: None,
                resource_root: None,
                capture_backend: None,
                dry_run: false,
            },
            args: vec![
                "--zip".to_string(),
                temp.path().join("missing.zip").display().to_string(),
                "--out".to_string(),
                temp.path().join("out.zip").display().to_string(),
            ],
            lease: None,
            created_at_unix_ms: 1,
        };

        let err = execute_session_command_request_inner(&request, temp.path()).unwrap_err();
        assert_eq!(err.code, "lab_lease_required");
        assert_eq!(err.exit_code(), 3);
    }

    #[test]
    fn session_operation_run_request_requires_lease_before_device_io() {
        let temp = TempDir::new().unwrap();
        let request = SessionCommandRequest {
            request_id: "request-1".to_string(),
            command: "operation_run".to_string(),
            global: SessionCommandGlobal {
                instance: Some("ak".to_string()),
                game: None,
                server: None,
                resource_root: None,
                capture_backend: None,
                dry_run: false,
            },
            args: vec![
                "--operation-dir".to_string(),
                temp.path().join("missing-operation").display().to_string(),
            ],
            lease: None,
            created_at_unix_ms: 1,
        };

        let err = execute_session_command_request_inner(&request, temp.path()).unwrap_err();
        assert_eq!(err.code, "lab_lease_required");
        assert_eq!(err.exit_code(), 3);
    }

    #[test]
    fn session_instance_connectivity_requests_require_lease_before_device_io() {
        for action in ["connect", "reconnect"] {
            let temp = TempDir::new().unwrap();
            let request = SessionCommandRequest {
                request_id: format!("{action}-request"),
                command: "instance".to_string(),
                global: SessionCommandGlobal {
                    instance: Some("ak".to_string()),
                    game: None,
                    server: None,
                    resource_root: None,
                    capture_backend: None,
                    dry_run: false,
                },
                args: vec![action.to_string()],
                lease: None,
                created_at_unix_ms: 1,
            };

            let err = execute_session_command_request_inner(&request, temp.path()).unwrap_err();
            assert_eq!(err.code, "lab_lease_required");
            assert_eq!(err.exit_code(), 3);
        }
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
    fn session_status_request_returns_daemon_diagnostics() {
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let config_path = temp.path().join("config.json");
        unsafe {
            env::set_var(CONFIG_ENV, &config_path);
        }
        let state_dir = temp.path();
        write_user_config(&UserConfig::default()).unwrap();
        let info = SessionInfo {
            pid: 123,
            started_at_unix_ms: 10,
            state_dir: state_dir.display().to_string(),
            runtime_version: RUNTIME_VERSION.to_string(),
        };
        let heartbeat = SessionHeartbeat {
            pid: 123,
            updated_at_unix_ms: 20,
            state: "idle".to_string(),
        };
        write_json_file_atomic(&session_info_path(state_dir), &info).unwrap();
        write_json_file_atomic(&session_heartbeat_path(state_dir), &heartbeat).unwrap();
        let request = SessionCommandRequest {
            request_id: "request-status".to_string(),
            command: "status".to_string(),
            global: SessionCommandGlobal {
                instance: Some("ak".to_string()),
                game: None,
                server: None,
                resource_root: None,
                capture_backend: None,
                dry_run: false,
            },
            args: vec!["--diagnostics".to_string()],
            lease: None,
            created_at_unix_ms: 1,
        };

        let status = execute_session_command_request_inner(&request, state_dir).unwrap();
        unsafe {
            env::remove_var(CONFIG_ENV);
        }

        assert_eq!(status.get("running").and_then(Value::as_bool), Some(true));
        assert_eq!(
            status.pointer("/info/pid").and_then(Value::as_u64),
            Some(123)
        );
        assert_eq!(
            status
                .pointer("/diagnostics/queues/pending_requests")
                .and_then(Value::as_u64),
            Some(0)
        );
        assert_eq!(
            status
                .pointer("/diagnostics/liveness/status")
                .and_then(Value::as_str),
            Some("stale")
        );
        assert_eq!(
            status
                .pointer("/diagnostics/journal/retention/max_bytes")
                .and_then(Value::as_u64),
            Some(SESSION_REQUEST_JOURNAL_MAX_BYTES)
        );
        assert_eq!(
            status
                .pointer("/diagnostics/instances/available")
                .and_then(Value::as_bool),
            Some(true)
        );
    }

    #[test]
    fn session_status_diagnostics_reports_active_leases() {
        let temp = TempDir::new().unwrap();
        let state_dir = temp.path();
        let lease = new_session_lease(
            "ak".to_string(),
            "scheduler".to_string(),
            Some("lease-1".to_string()),
            false,
            None,
        );
        write_json_file_atomic(&session_lease_path(state_dir, "ak"), &lease).unwrap();

        let status = session_status_payload(state_dir, true).unwrap();

        assert_eq!(
            status
                .pointer("/diagnostics/leases/active_count")
                .and_then(Value::as_u64),
            Some(1)
        );
        assert_eq!(
            status
                .pointer("/diagnostics/leases/leases/0/instance")
                .and_then(Value::as_str),
            Some("ak")
        );
        assert_eq!(
            status
                .pointer("/diagnostics/leases/leases/0/holder")
                .and_then(Value::as_str),
            Some("scheduler")
        );
        assert_eq!(
            status
                .pointer("/diagnostics/leases/leases/0/lease_id")
                .and_then(Value::as_str),
            Some("lease-1")
        );
    }

    #[test]
    fn session_status_diagnostics_marks_instance_registry_unavailable_without_config() {
        let temp = TempDir::new().unwrap();

        let status = session_status_payload(temp.path(), true).unwrap();

        assert_eq!(
            status
                .pointer("/diagnostics/instances/available")
                .and_then(Value::as_bool),
            Some(false)
        );
        assert_eq!(
            status
                .pointer("/diagnostics/instances/count")
                .and_then(Value::as_u64),
            Some(0)
        );
    }

    #[test]
    fn session_instance_registry_diagnostics_lists_configured_instances() {
        let mut config = UserConfig::default();
        config.instances.insert(
            "ak-b".to_string(),
            InstanceConfig {
                serial: Some("127.0.0.1:16416".to_string()),
                game: Some("ark".to_string()),
                server: Some("cn-bilibili".to_string()),
                package: Some("com.hypergryph.arknights.bilibili".to_string()),
                adb_path: Some("C:\\Tools\\adb.exe".to_string()),
                capture_backend: Some("nemu_ipc".to_string()),
            },
        );

        let diagnostics = session_instance_registry_diagnostics(Some(&config));

        assert_eq!(
            diagnostics.get("available").and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(diagnostics.get("count").and_then(Value::as_u64), Some(1));
        assert_eq!(
            diagnostics
                .pointer("/instances/0/id")
                .and_then(Value::as_str),
            Some("ak-b")
        );
        assert_eq!(
            diagnostics
                .pointer("/instances/0/serial")
                .and_then(Value::as_str),
            Some("127.0.0.1:16416")
        );
        assert_eq!(
            diagnostics
                .pointer("/instances/0/package_configured")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            diagnostics
                .pointer("/instances/0/adb_path")
                .and_then(Value::as_str),
            Some("C:\\Tools\\adb.exe")
        );
        assert_eq!(
            diagnostics
                .pointer("/instances/0/capture_backend")
                .and_then(Value::as_str),
            Some("nemu_ipc")
        );
    }

    #[test]
    fn session_status_diagnostics_recommends_start_when_stopped() {
        let temp = TempDir::new().unwrap();

        let status = session_status_payload(temp.path(), true).unwrap();
        let actions = status
            .pointer("/diagnostics/recommended_actions")
            .and_then(Value::as_array)
            .unwrap();

        assert_eq!(actions.len(), 1);
        assert_eq!(
            actions[0].get("action").and_then(Value::as_str),
            Some("start_session")
        );
        assert_eq!(
            actions[0].pointer("/args/0").and_then(Value::as_str),
            Some("session")
        );
        assert_eq!(
            actions[0].pointer("/args/1").and_then(Value::as_str),
            Some("start")
        );
    }

    #[test]
    fn session_status_diagnostics_has_no_recommendations_when_alive() {
        let temp = TempDir::new().unwrap();
        write_test_session_files(temp.path());

        let status = session_status_payload(temp.path(), true).unwrap();
        let actions = status
            .pointer("/diagnostics/recommended_actions")
            .and_then(Value::as_array)
            .unwrap();

        assert!(actions.is_empty());
    }

    #[test]
    fn session_status_diagnostics_recommends_stale_cleanup_sequence() {
        let temp = TempDir::new().unwrap();
        write_test_session_info_only(temp.path());
        write_test_session_heartbeat(
            temp.path(),
            321,
            current_unix_ms().saturating_sub(SESSION_HEARTBEAT_STALE_MS + 1),
        );

        let status = session_status_payload(temp.path(), true).unwrap();
        let actions = status
            .pointer("/diagnostics/recommended_actions")
            .and_then(Value::as_array)
            .unwrap();

        assert_eq!(actions.len(), 3);
        assert_eq!(
            actions[0].get("action").and_then(Value::as_str),
            Some("inspect_stale_cleanup")
        );
        assert_eq!(
            actions[0].pointer("/args/1").and_then(Value::as_str),
            Some("cleanup")
        );
        assert_eq!(
            actions[0].pointer("/args/3").and_then(Value::as_str),
            Some("--dry-run")
        );
        assert_eq!(
            actions[1].get("action").and_then(Value::as_str),
            Some("cleanup_stale_session")
        );
        assert_eq!(
            actions[2].get("action").and_then(Value::as_str),
            Some("start_session")
        );
    }

    #[test]
    fn session_status_diagnostics_rejects_corrupt_lease_file() {
        let temp = TempDir::new().unwrap();
        let path = session_lease_path(temp.path(), "ak");
        fs::write(&path, "{not-json").unwrap();

        let err = session_status_payload(temp.path(), true).unwrap_err();

        assert_eq!(err.code, "validation_failed");
        assert!(err.message.contains("failed to parse"));
        assert!(err.message.contains("lease-ak.json"));
    }

    #[test]
    fn session_stream_input_relay_request_requires_lease_metadata() {
        let temp = TempDir::new().unwrap();
        let request = SessionCommandRequest {
            request_id: "stream-input-relay".to_string(),
            command: "stream".to_string(),
            global: SessionCommandGlobal {
                instance: Some("ak".to_string()),
                game: None,
                server: None,
                resource_root: None,
                capture_backend: None,
                dry_run: true,
            },
            args: vec![
                "--dry-run".to_string(),
                "--input-relay".to_string(),
                "tap".to_string(),
                "10".to_string(),
                "20".to_string(),
            ],
            lease: None,
            created_at_unix_ms: 1,
        };

        let err = execute_session_command_request_inner(&request, temp.path()).unwrap_err();

        assert_eq!(err.code, "lab_lease_required");
    }

    #[test]
    fn session_stream_input_relay_request_accepts_matching_lease() {
        let temp = TempDir::new().unwrap();
        let state_dir = temp.path();
        let lease = new_session_lease(
            "ak".to_string(),
            "scheduler".to_string(),
            Some("lease-1".to_string()),
            false,
            None,
        );
        write_json_file_atomic(&session_lease_path(state_dir, "ak"), &lease).unwrap();
        let request = SessionCommandRequest {
            request_id: "stream-input-relay".to_string(),
            command: "stream".to_string(),
            global: SessionCommandGlobal {
                instance: Some("ak".to_string()),
                game: None,
                server: None,
                resource_root: None,
                capture_backend: None,
                dry_run: true,
            },
            args: vec![
                "--dry-run".to_string(),
                "--max-frames".to_string(),
                "1".to_string(),
                "--input-event".to_string(),
                "tap,10,20".to_string(),
                "--input-event".to_string(),
                "key,back".to_string(),
            ],
            lease: Some(SessionCommandLease {
                holder: "scheduler".to_string(),
                lease_id: Some("lease-1".to_string()),
            }),
            created_at_unix_ms: 1,
        };

        let payload = execute_session_command_request_inner(&request, state_dir).unwrap();

        assert_eq!(
            payload
                .pointer("/input_relay/status")
                .and_then(Value::as_str),
            Some("planned")
        );
        assert_eq!(
            payload
                .pointer("/input_relay/action_count")
                .and_then(Value::as_u64),
            Some(2)
        );
        assert_eq!(
            payload
                .pointer("/input_relay/actions/1/key")
                .and_then(Value::as_str),
            Some("4")
        );
    }

    #[test]
    fn session_journal_request_returns_daemon_journal_entries() {
        let temp = TempDir::new().unwrap();
        let state_dir = temp.path();
        let journaled = SessionCommandRequest {
            request_id: "journaled-request".to_string(),
            command: "stream".to_string(),
            global: SessionCommandGlobal {
                instance: Some("ak".to_string()),
                game: None,
                server: None,
                resource_root: None,
                capture_backend: None,
                dry_run: false,
            },
            args: vec![
                "--dry-run".to_string(),
                "--max-frames".to_string(),
                "1".to_string(),
            ],
            lease: None,
            created_at_unix_ms: 1,
        };
        let response = SessionCommandResponse {
            request_id: "journaled-request".to_string(),
            command: "stream".to_string(),
            ok: true,
            data: Some(json!({"status": "done"})),
            error: None,
            started_at_unix_ms: 2,
            completed_at_unix_ms: 3,
        };
        append_session_request_journal(state_dir, &journaled, &response).unwrap();
        let query = SessionCommandRequest {
            request_id: "journal-query".to_string(),
            command: "journal".to_string(),
            global: journaled.global.clone(),
            args: vec!["--limit".to_string(), "1".to_string()],
            lease: None,
            created_at_unix_ms: 4,
        };

        let payload = execute_session_command_request_inner(&query, state_dir).unwrap();

        assert_eq!(payload.get("limit").and_then(Value::as_u64), Some(1));
        assert_eq!(
            payload
                .pointer("/entries/0/request_id")
                .and_then(Value::as_str),
            Some("journaled-request")
        );
        assert_eq!(
            payload.pointer("/entries/0/ok").and_then(Value::as_bool),
            Some(true)
        );
    }

    #[test]
    fn session_events_request_returns_stable_request_events() {
        let temp = TempDir::new().unwrap();
        let state_dir = temp.path();
        let journaled = SessionCommandRequest {
            request_id: "evented-request".to_string(),
            command: "contract".to_string(),
            global: SessionCommandGlobal {
                instance: Some("ak".to_string()),
                game: None,
                server: None,
                resource_root: None,
                capture_backend: None,
                dry_run: false,
            },
            args: Vec::new(),
            lease: Some(SessionCommandLease {
                holder: "tester".to_string(),
                lease_id: Some("lease-1".to_string()),
            }),
            created_at_unix_ms: 10,
        };
        let response = SessionCommandResponse {
            request_id: "evented-request".to_string(),
            command: "contract".to_string(),
            ok: true,
            data: Some(json!({"status": "done"})),
            error: None,
            started_at_unix_ms: 15,
            completed_at_unix_ms: 45,
        };
        append_session_request_journal(state_dir, &journaled, &response).unwrap();
        let query = SessionCommandRequest {
            request_id: "events-query".to_string(),
            command: "events".to_string(),
            global: journaled.global.clone(),
            args: vec!["--limit".to_string(), "1".to_string()],
            lease: None,
            created_at_unix_ms: 50,
        };

        let payload = execute_session_command_request_inner(&query, state_dir).unwrap();

        assert_eq!(
            payload.get("schema_version").and_then(Value::as_str),
            Some("session.events.v0.1")
        );
        assert_eq!(payload.get("event_count").and_then(Value::as_u64), Some(1));
        assert_eq!(
            payload.pointer("/events/0/type").and_then(Value::as_str),
            Some("session.request.completed")
        );
        assert_eq!(
            payload
                .pointer("/events/0/request_id")
                .and_then(Value::as_str),
            Some("evented-request")
        );
        assert_eq!(
            payload.pointer("/events/0/command").and_then(Value::as_str),
            Some("contract")
        );
        assert_eq!(
            payload
                .pointer("/events/0/timing/queue_wait_ms")
                .and_then(Value::as_u64),
            Some(5)
        );
        assert_eq!(
            payload
                .pointer("/events/0/timing/duration_ms")
                .and_then(Value::as_u64),
            Some(30)
        );
        assert_eq!(
            payload
                .pointer("/events/0/lease/holder")
                .and_then(Value::as_str),
            Some("tester")
        );
    }

    #[test]
    fn session_events_after_unix_ms_returns_incremental_cursor_window() {
        let temp = TempDir::new().unwrap();
        let state_dir = temp.path();
        let global = SessionCommandGlobal {
            instance: Some("ak".to_string()),
            game: None,
            server: None,
            resource_root: None,
            capture_backend: None,
            dry_run: false,
        };
        let first = SessionCommandRequest {
            request_id: "event-1".to_string(),
            command: "status".to_string(),
            global: global.clone(),
            args: Vec::new(),
            lease: None,
            created_at_unix_ms: 10,
        };
        let first_response = SessionCommandResponse {
            request_id: "event-1".to_string(),
            command: "status".to_string(),
            ok: true,
            data: Some(json!({"status": "ok"})),
            error: None,
            started_at_unix_ms: 11,
            completed_at_unix_ms: 30,
        };
        append_session_request_journal(state_dir, &first, &first_response).unwrap();
        let second = SessionCommandRequest {
            request_id: "event-2".to_string(),
            command: "events".to_string(),
            global,
            args: Vec::new(),
            lease: None,
            created_at_unix_ms: 40,
        };
        let second_response = SessionCommandResponse {
            request_id: "event-2".to_string(),
            command: "events".to_string(),
            ok: true,
            data: Some(json!({"status": "ok"})),
            error: None,
            started_at_unix_ms: 41,
            completed_at_unix_ms: 70,
        };
        append_session_request_journal(state_dir, &second, &second_response).unwrap();

        let query = SessionCommandRequest {
            request_id: "events-after-query".to_string(),
            command: "events".to_string(),
            global: second.global.clone(),
            args: vec![
                "--limit".to_string(),
                "10".to_string(),
                "--after-unix-ms".to_string(),
                "30".to_string(),
            ],
            lease: None,
            created_at_unix_ms: 80,
        };

        let payload = execute_session_command_request_inner(&query, state_dir).unwrap();
        let events = payload.get("events").and_then(Value::as_array).unwrap();

        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].get("request_id").and_then(Value::as_str),
            Some("event-2")
        );
        assert_eq!(
            payload.get("after_unix_ms").and_then(Value::as_u64),
            Some(30)
        );
        assert_eq!(
            payload
                .pointer("/cursor/latest_timestamp_unix_ms")
                .and_then(Value::as_u64),
            Some(70)
        );
        assert_eq!(
            payload
                .pointer("/cursor/next_after_unix_ms")
                .and_then(Value::as_u64),
            Some(70)
        );
        assert_eq!(
            payload
                .pointer("/cursor/next_after_request_id")
                .and_then(Value::as_str),
            Some("event-2")
        );
    }

    #[test]
    fn session_events_after_request_id_returns_same_timestamp_later_events() {
        let temp = TempDir::new().unwrap();
        let state_dir = temp.path();
        let global = SessionCommandGlobal {
            instance: Some("ak".to_string()),
            game: None,
            server: None,
            resource_root: None,
            capture_backend: None,
            dry_run: false,
        };
        for request_id in ["event-a", "event-b"] {
            let request = SessionCommandRequest {
                request_id: request_id.to_string(),
                command: "status".to_string(),
                global: global.clone(),
                args: Vec::new(),
                lease: None,
                created_at_unix_ms: 10,
            };
            let response = SessionCommandResponse {
                request_id: request_id.to_string(),
                command: "status".to_string(),
                ok: true,
                data: Some(json!({"status": "ok"})),
                error: None,
                started_at_unix_ms: 20,
                completed_at_unix_ms: 70,
            };
            append_session_request_journal(state_dir, &request, &response).unwrap();
        }
        let query = SessionCommandRequest {
            request_id: "events-after-request-query".to_string(),
            command: "events".to_string(),
            global,
            args: vec![
                "--limit".to_string(),
                "10".to_string(),
                "--after-request-id".to_string(),
                "event-a".to_string(),
            ],
            lease: None,
            created_at_unix_ms: 80,
        };

        let payload = execute_session_command_request_inner(&query, state_dir).unwrap();
        let events = payload.get("events").and_then(Value::as_array).unwrap();

        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].get("request_id").and_then(Value::as_str),
            Some("event-b")
        );
        assert_eq!(
            payload.get("after_request_id").and_then(Value::as_str),
            Some("event-a")
        );
        assert_eq!(
            payload
                .pointer("/cursor/latest_timestamp_unix_ms")
                .and_then(Value::as_u64),
            Some(70)
        );
        assert_eq!(
            payload
                .pointer("/cursor/latest_request_id")
                .and_then(Value::as_str),
            Some("event-b")
        );
    }

    #[test]
    fn session_events_after_request_id_missing_fails_visibly() {
        let temp = TempDir::new().unwrap();
        let state_dir = temp.path();
        let global = SessionCommandGlobal {
            instance: Some("ak".to_string()),
            game: None,
            server: None,
            resource_root: None,
            capture_backend: None,
            dry_run: false,
        };
        let request = SessionCommandRequest {
            request_id: "event-a".to_string(),
            command: "status".to_string(),
            global: global.clone(),
            args: Vec::new(),
            lease: None,
            created_at_unix_ms: 10,
        };
        let response = SessionCommandResponse {
            request_id: "event-a".to_string(),
            command: "status".to_string(),
            ok: true,
            data: Some(json!({"status": "ok"})),
            error: None,
            started_at_unix_ms: 20,
            completed_at_unix_ms: 70,
        };
        append_session_request_journal(state_dir, &request, &response).unwrap();
        let query = SessionCommandRequest {
            request_id: "events-missing-request-query".to_string(),
            command: "events".to_string(),
            global,
            args: vec![
                "--after-request-id".to_string(),
                "missing-request".to_string(),
            ],
            lease: None,
            created_at_unix_ms: 80,
        };

        let err = execute_session_command_request_inner(&query, state_dir).unwrap_err();

        assert_eq!(err.code, "event_cursor_not_found");
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn session_capabilities_request_returns_daemon_contract() {
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        unsafe {
            env::set_var(CONFIG_ENV, &config);
        }
        let query = SessionCommandRequest {
            request_id: "capabilities-query".to_string(),
            command: "capabilities".to_string(),
            global: SessionCommandGlobal {
                instance: Some("ak".to_string()),
                game: None,
                server: None,
                resource_root: None,
                capture_backend: None,
                dry_run: false,
            },
            args: Vec::new(),
            lease: None,
            created_at_unix_ms: 4,
        };

        let payload = execute_session_command_request_inner(&query, temp.path()).unwrap();
        unsafe {
            env::remove_var(CONFIG_ENV);
        }

        assert_eq!(
            payload
                .pointer("/session_layer/schema_version")
                .and_then(Value::as_str),
            Some("session.capabilities.v0.1")
        );
        assert_eq!(
            payload
                .pointer("/session_layer/resident_daemon/request_command")
                .and_then(Value::as_str),
            Some("session request capabilities")
        );
        assert_eq!(
            payload
                .pointer("/session_layer/safety/session_layer_only_throat")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert!(
            payload
                .get("commands")
                .and_then(Value::as_array)
                .unwrap()
                .iter()
                .any(|command| command.get("command").and_then(Value::as_str)
                    == Some("session request capabilities"))
        );
        assert!(
            payload
                .get("commands")
                .and_then(Value::as_array)
                .unwrap()
                .iter()
                .any(|command| command.get("command").and_then(Value::as_str)
                    == Some("session request transport"))
        );
        assert!(
            payload
                .get("commands")
                .and_then(Value::as_array)
                .unwrap()
                .iter()
                .any(|command| command.get("command").and_then(Value::as_str)
                    == Some("session request instance registry"))
        );
        assert!(
            payload
                .get("commands")
                .and_then(Value::as_array)
                .unwrap()
                .iter()
                .any(|command| command.get("command").and_then(Value::as_str)
                    == Some("session request instance keep-alive"))
        );
        assert!(
            payload
                .get("commands")
                .and_then(Value::as_array)
                .unwrap()
                .iter()
                .any(|command| command.get("command").and_then(Value::as_str)
                    == Some("session request instance connect"))
        );
    }

    #[test]
    fn session_instance_registry_request_returns_contract() {
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let config_path = temp.path().join("config.json");
        unsafe {
            env::set_var(CONFIG_ENV, &config_path);
        }
        let mut config = UserConfig::default();
        config.instances.insert(
            "ak-b".to_string(),
            InstanceConfig {
                serial: Some("127.0.0.1:16416".to_string()),
                game: Some("ark".to_string()),
                server: Some("cn-bilibili".to_string()),
                package: Some("com.hypergryph.arknights.bilibili".to_string()),
                adb_path: None,
                capture_backend: Some("nemu_ipc".to_string()),
            },
        );
        write_user_config(&config).unwrap();
        let query = SessionCommandRequest {
            request_id: "instance-registry-query".to_string(),
            command: "instance".to_string(),
            global: SessionCommandGlobal {
                instance: Some("ak-b".to_string()),
                game: None,
                server: None,
                resource_root: None,
                capture_backend: None,
                dry_run: false,
            },
            args: vec!["registry".to_string()],
            lease: None,
            created_at_unix_ms: 4,
        };

        let payload = execute_session_command_request_inner(&query, temp.path()).unwrap();
        unsafe {
            env::remove_var(CONFIG_ENV);
        }

        assert_eq!(
            payload.get("schema_version").and_then(Value::as_str),
            Some("session.instance_registry.v0.1")
        );
        assert_eq!(payload.get("count").and_then(Value::as_u64), Some(1));
        assert_eq!(
            payload
                .pointer("/instances/0/effective/capture_backend")
                .and_then(Value::as_str),
            Some("nemu_ipc")
        );
    }

    #[test]
    fn session_contract_request_returns_access_contract() {
        let query = SessionCommandRequest {
            request_id: "contract-query".to_string(),
            command: "contract".to_string(),
            global: SessionCommandGlobal {
                instance: Some("ak".to_string()),
                game: None,
                server: None,
                resource_root: None,
                capture_backend: None,
                dry_run: false,
            },
            args: Vec::new(),
            lease: None,
            created_at_unix_ms: 4,
        };

        let temp = TempDir::new().unwrap();
        let payload = execute_session_command_request_inner(&query, temp.path()).unwrap();

        assert_eq!(
            payload.get("schema_version").and_then(Value::as_str),
            Some("session.access.v0.1")
        );
        assert_eq!(
            payload
                .pointer("/entrypoints/trusted_remote/encryption_required")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            payload
                .pointer("/session_layer/ui_direct_device_access_allowed")
                .and_then(Value::as_bool),
            Some(false)
        );
        assert_eq!(
            payload
                .pointer("/request_classes/control/requires_lease")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            payload
                .pointer("/daemon_queries/api")
                .and_then(Value::as_str),
            Some("session request api")
        );
        assert_eq!(
            payload
                .pointer("/daemon_queries/transport")
                .and_then(Value::as_str),
            Some("session request transport")
        );
        assert_eq!(
            payload
                .pointer("/daemon_queries/instance_registry")
                .and_then(Value::as_str),
            Some("session request instance registry")
        );
        assert_eq!(
            payload
                .pointer("/daemon_queries/instance_health")
                .and_then(Value::as_str),
            Some("session request instance health")
        );
        assert_eq!(
            payload
                .pointer("/daemon_queries/instance_keep_alive")
                .and_then(Value::as_str),
            Some("session request instance keep-alive")
        );
        assert_eq!(
            payload
                .pointer("/daemon_controls/app_lifecycle")
                .and_then(Value::as_str),
            Some("session request app <launch|stop|restart>")
        );
        assert_eq!(
            payload
                .pointer("/daemon_controls/instance_app_lifecycle")
                .and_then(Value::as_str),
            Some("session request instance app <launch|stop|restart>")
        );
        assert_eq!(
            payload
                .pointer("/daemon_controls/instance_connect")
                .and_then(Value::as_str),
            Some("session request instance connect")
        );
    }

    #[test]
    fn session_api_request_returns_api_contract() {
        let query = SessionCommandRequest {
            request_id: "api-query".to_string(),
            command: "api".to_string(),
            global: SessionCommandGlobal {
                instance: Some("ak".to_string()),
                game: None,
                server: None,
                resource_root: None,
                capture_backend: None,
                dry_run: false,
            },
            args: Vec::new(),
            lease: None,
            created_at_unix_ms: 4,
        };

        let temp = TempDir::new().unwrap();
        let payload = execute_session_command_request_inner(&query, temp.path()).unwrap();

        assert_eq!(
            payload.get("schema_version").and_then(Value::as_str),
            Some("session.api.v0.1")
        );
        assert_eq!(
            payload
                .pointer("/session_layer/clients_must_not_directly_touch_adb_or_devices")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            payload
                .pointer("/access_channels/trusted_remote/network_listener_implemented")
                .and_then(Value::as_bool),
            Some(false)
        );
        assert_eq!(
            payload
                .pointer("/daemon_request_queue/submit_command")
                .and_then(Value::as_str),
            Some("session request <command>")
        );
        assert_eq!(
            payload
                .pointer("/command_classes/control/requires_lease")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            payload
                .pointer("/envelopes/event_view/filters/1")
                .and_then(Value::as_str),
            Some("--after-unix-ms")
        );
        assert_eq!(
            payload
                .pointer("/envelopes/event_view/filters/2")
                .and_then(Value::as_str),
            Some("--after-request-id")
        );
        assert_eq!(
            payload
                .pointer("/envelopes/event_view/cursor_fields/2")
                .and_then(Value::as_str),
            Some("latest_request_id")
        );
        assert_eq!(
            payload
                .pointer("/envelopes/event_view/cursor_error")
                .and_then(Value::as_str),
            Some("event_cursor_not_found")
        );
        assert_eq!(
            payload
                .pointer("/envelopes/transport_view/schema_version")
                .and_then(Value::as_str),
            Some("session.transport.v0.1")
        );
        assert_eq!(
            payload
                .pointer("/envelopes/instance_registry_view/schema_version")
                .and_then(Value::as_str),
            Some("session.instance_registry.v0.1")
        );
        assert_eq!(
            payload
                .pointer("/envelopes/instance_registry_view/daemon_query")
                .and_then(Value::as_str),
            Some("session request instance registry")
        );
        assert_eq!(
            payload
                .pointer("/envelopes/instance_health_view/daemon_query")
                .and_then(Value::as_str),
            Some("session request instance health [--capture-diagnose]")
        );
        assert_eq!(
            payload
                .pointer("/envelopes/instance_keep_alive_view/daemon_query")
                .and_then(Value::as_str),
            Some("session request instance keep-alive")
        );
        assert_eq!(
            payload
                .pointer("/envelopes/instance_connect_view/daemon_query")
                .and_then(Value::as_str),
            Some("session request instance connect")
        );
        assert_eq!(
            payload
                .pointer("/envelopes/instance_connect_view/requires_lease")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            payload
                .pointer("/envelopes/app_lifecycle_view/daemon_query")
                .and_then(Value::as_str),
            Some("session request app <launch|stop|restart>")
        );
        assert_eq!(
            payload
                .pointer("/envelopes/app_lifecycle_view/aliases/0")
                .and_then(Value::as_str),
            Some("session instance app <launch|stop|restart>")
        );
        assert_eq!(
            payload
                .pointer("/envelopes/app_lifecycle_view/aliases/1")
                .and_then(Value::as_str),
            Some("session request instance app <launch|stop|restart>")
        );
        assert_eq!(
            payload
                .pointer("/envelopes/app_lifecycle_view/requires_lease")
                .and_then(Value::as_bool),
            Some(true)
        );
    }

    #[test]
    fn session_transport_request_returns_transport_contract() {
        let query = SessionCommandRequest {
            request_id: "transport-query".to_string(),
            command: "transport".to_string(),
            global: SessionCommandGlobal {
                instance: Some("ak".to_string()),
                game: None,
                server: None,
                resource_root: None,
                capture_backend: None,
                dry_run: false,
            },
            args: Vec::new(),
            lease: None,
            created_at_unix_ms: 4,
        };

        let temp = TempDir::new().unwrap();
        let payload = execute_session_command_request_inner(&query, temp.path()).unwrap();

        assert_eq!(
            payload.get("schema_version").and_then(Value::as_str),
            Some("session.transport.v0.1")
        );
        assert_eq!(
            payload
                .pointer("/channels/local_cli/status")
                .and_then(Value::as_str),
            Some("available")
        );
        assert_eq!(
            payload
                .pointer("/channels/daemon_file_ipc/serialized_by_daemon")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            payload
                .pointer("/channels/trusted_remote/network_listener_implemented")
                .and_then(Value::as_bool),
            Some(false)
        );
        assert_eq!(
            payload
                .pointer("/channels/interactive_stream/frame_event_schema")
                .and_then(Value::as_str),
            Some("session.stream.event.v0.1")
        );
        assert_eq!(
            payload
                .pointer("/safety/clients_must_not_directly_touch_adb_or_devices")
                .and_then(Value::as_bool),
            Some(true)
        );
    }

    #[test]
    fn session_lease_request_acquires_and_releases_in_daemon_state_dir() {
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("daemon-state");
        unsafe {
            env::set_var(CONFIG_ENV, &config);
        }
        let global = SessionCommandGlobal {
            instance: Some("ak".to_string()),
            game: None,
            server: None,
            resource_root: None,
            capture_backend: None,
            dry_run: false,
        };
        let acquire = SessionCommandRequest {
            request_id: "lease-acquire".to_string(),
            command: "lease".to_string(),
            global: global.clone(),
            args: vec![
                "acquire".to_string(),
                "--holder".to_string(),
                "scheduler".to_string(),
                "--lease-id".to_string(),
                "lease-1".to_string(),
            ],
            lease: None,
            created_at_unix_ms: 1,
        };

        let acquired = execute_session_command_request_inner(&acquire, &state_dir).unwrap();

        assert_eq!(
            acquired.get("status").and_then(Value::as_str),
            Some("acquired")
        );
        assert_eq!(
            acquired.pointer("/lease/holder").and_then(Value::as_str),
            Some("scheduler")
        );
        assert!(session_lease_path(&state_dir, "ak").exists());

        let release = SessionCommandRequest {
            request_id: "lease-release".to_string(),
            command: "lease".to_string(),
            global,
            args: vec![
                "release".to_string(),
                "--holder".to_string(),
                "scheduler".to_string(),
                "--lease-id".to_string(),
                "lease-1".to_string(),
            ],
            lease: None,
            created_at_unix_ms: 2,
        };
        let released = execute_session_command_request_inner(&release, &state_dir).unwrap();
        unsafe {
            env::remove_var(CONFIG_ENV);
        }

        assert_eq!(
            released.get("status").and_then(Value::as_str),
            Some("released")
        );
        assert!(!session_lease_path(&state_dir, "ak").exists());
    }

    #[test]
    fn session_record_request_starts_statuses_and_stops_in_daemon_state_dir() {
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("daemon-state");
        unsafe {
            env::set_var(CONFIG_ENV, &config);
        }
        let global = SessionCommandGlobal {
            instance: Some("ak".to_string()),
            game: None,
            server: None,
            resource_root: None,
            capture_backend: None,
            dry_run: false,
        };
        let start = SessionCommandRequest {
            request_id: "record-start".to_string(),
            command: "record".to_string(),
            global: global.clone(),
            args: vec![
                "start".to_string(),
                "--task-id".to_string(),
                "task_alpha".to_string(),
                "--holder".to_string(),
                "scheduler".to_string(),
                "--lease-id".to_string(),
                "lease-1".to_string(),
            ],
            lease: None,
            created_at_unix_ms: 1,
        };

        let started = execute_session_command_request_inner(&start, &state_dir).unwrap();

        assert_eq!(
            started.get("status").and_then(Value::as_str),
            Some("started")
        );
        assert_eq!(
            started.pointer("/record/holder").and_then(Value::as_str),
            Some("scheduler")
        );
        assert!(session_record_path(&state_dir, "ak").exists());

        let status = SessionCommandRequest {
            request_id: "record-status".to_string(),
            command: "record".to_string(),
            global: global.clone(),
            args: vec!["status".to_string()],
            lease: None,
            created_at_unix_ms: 2,
        };
        let status_payload = execute_session_command_request_inner(&status, &state_dir).unwrap();

        assert_eq!(
            status_payload
                .pointer("/record/task_id")
                .and_then(Value::as_str),
            Some("task_alpha")
        );

        let stop = SessionCommandRequest {
            request_id: "record-stop".to_string(),
            command: "record".to_string(),
            global,
            args: vec!["stop".to_string()],
            lease: None,
            created_at_unix_ms: 3,
        };
        let stopped = execute_session_command_request_inner(&stop, &state_dir).unwrap();
        unsafe {
            env::remove_var(CONFIG_ENV);
        }

        assert_eq!(
            stopped.get("status").and_then(Value::as_str),
            Some("stopped")
        );
        assert_eq!(
            stopped.pointer("/record/status").and_then(Value::as_str),
            Some("stopped")
        );
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
    fn status_via_daemon_without_daemon_is_runtime_error() {
        let temp = TempDir::new().unwrap();
        let result = run_cli(
            [
                "--json",
                "status",
                "--via-daemon",
                "--diagnostics",
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
    fn session_request_status_without_daemon_is_runtime_error() {
        let temp = TempDir::new().unwrap();
        let result = run_cli(
            [
                "--json",
                "session",
                "request",
                "status",
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
    fn session_request_journal_without_daemon_is_runtime_error() {
        let temp = TempDir::new().unwrap();
        let result = run_cli(
            [
                "--json",
                "session",
                "request",
                "journal",
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
    fn devices_via_daemon_without_daemon_is_runtime_error() {
        let temp = TempDir::new().unwrap();
        let result = run_cli(
            [
                "--json",
                "devices",
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
    fn session_request_devices_without_daemon_is_runtime_error() {
        let temp = TempDir::new().unwrap();
        let result = run_cli(
            [
                "--json",
                "session",
                "request",
                "devices",
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
    fn session_request_lease_without_daemon_is_runtime_error() {
        let temp = TempDir::new().unwrap();
        let result = run_cli(
            [
                "--json",
                "session",
                "request",
                "lease",
                "status",
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
    fn session_request_record_without_daemon_is_runtime_error() {
        let temp = TempDir::new().unwrap();
        let result = run_cli(
            [
                "--json",
                "session",
                "request",
                "record",
                "status",
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
    fn capture_via_daemon_without_daemon_is_runtime_error() {
        let temp = TempDir::new().unwrap();
        let out = temp.path().join("frame.png");
        let result = run_cli(
            [
                "--json",
                "capture",
                "--via-daemon",
                "--state-dir",
                temp.path().to_str().unwrap(),
                "--out",
                out.to_str().unwrap(),
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
    fn stream_via_daemon_without_daemon_is_runtime_error() {
        let temp = TempDir::new().unwrap();
        let result = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "stream",
                "--via-daemon",
                "--state-dir",
                temp.path().to_str().unwrap(),
                "--max-frames",
                "1",
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
        for args in [
            vec!["session", "app", "launch"],
            vec!["session", "instance", "app", "launch"],
        ] {
            let temp = TempDir::new().unwrap();
            let mut command = vec!["--json", "--instance", "ak"];
            command.extend(args);
            command.extend([
                "--via-daemon",
                "--state-dir",
                temp.path().to_str().unwrap(),
                "--lease-holder",
                "scheduler",
                "--lease-id",
                "lease-1",
                "--package",
                "com.example.game",
            ]);
            let result = run_cli(command, true);

            assert_eq!(result.exit_code(), 5);
            assert_eq!(
                result.envelope.error.as_ref().unwrap().code,
                "runtime_not_running"
            );
        }
    }

    #[test]
    fn session_instance_via_daemon_without_daemon_is_runtime_error() {
        let temp = TempDir::new().unwrap();
        let result = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "instance",
                "health",
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
    fn session_instance_connectivity_via_daemon_accepts_lease_flags_before_daemon_lookup() {
        for action in ["connect", "reconnect"] {
            let temp = TempDir::new().unwrap();
            let result = run_cli(
                [
                    "--json",
                    "--instance",
                    "ak",
                    "session",
                    "instance",
                    action,
                    "--via-daemon",
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
    }

    #[test]
    fn lab_run_via_daemon_accepts_lease_flags_before_daemon_lookup() {
        let temp = TempDir::new().unwrap();
        let input = temp.path().join("input.zip");
        let out = temp.path().join("out.zip");
        let result = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "lab",
                "run",
                "--via-daemon",
                "--state-dir",
                temp.path().to_str().unwrap(),
                "--lease-holder",
                "scheduler",
                "--lease-id",
                "lease-1",
                "--zip",
                input.to_str().unwrap(),
                "--out",
                out.to_str().unwrap(),
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
    fn package_run_via_daemon_accepts_lease_flags_before_daemon_lookup() {
        let temp = TempDir::new().unwrap();
        let input = temp.path().join("input.zip");
        let out = temp.path().join("out.zip");
        let result = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "package",
                "run",
                "--via-daemon",
                "--state-dir",
                temp.path().to_str().unwrap(),
                "--lease-holder",
                "scheduler",
                "--lease-id",
                "lease-1",
                "--zip",
                input.to_str().unwrap(),
                "--out",
                out.to_str().unwrap(),
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
    fn operation_run_via_daemon_accepts_lease_flags_before_daemon_lookup() {
        let temp = TempDir::new().unwrap();
        let operation_dir = temp.path().join("operation");
        let result = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "operation",
                "run",
                "--via-daemon",
                "--state-dir",
                temp.path().to_str().unwrap(),
                "--lease-holder",
                "scheduler",
                "--lease-id",
                "lease-1",
                "--operation-dir",
                operation_dir.to_str().unwrap(),
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
    fn session_request_instance_without_daemon_is_runtime_error() {
        let temp = TempDir::new().unwrap();
        let result = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "request",
                "instance",
                "health",
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
    fn session_request_capture_without_daemon_is_runtime_error() {
        let temp = TempDir::new().unwrap();
        let out = temp.path().join("frame.png");
        let result = run_cli(
            [
                "--json",
                "session",
                "request",
                "capture",
                "--state-dir",
                temp.path().to_str().unwrap(),
                "--out",
                out.to_str().unwrap(),
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
    fn session_request_lab_run_without_daemon_is_runtime_error() {
        let temp = TempDir::new().unwrap();
        let input = temp.path().join("input.zip");
        let out = temp.path().join("out.zip");
        let result = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "request",
                "lab-run",
                "--state-dir",
                temp.path().to_str().unwrap(),
                "--lease-holder",
                "scheduler",
                "--lease-id",
                "lease-1",
                "--zip",
                input.to_str().unwrap(),
                "--out",
                out.to_str().unwrap(),
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
    fn session_request_package_run_without_daemon_is_runtime_error() {
        let temp = TempDir::new().unwrap();
        let input = temp.path().join("input.zip");
        let out = temp.path().join("out.zip");
        let result = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "request",
                "package-run",
                "--state-dir",
                temp.path().to_str().unwrap(),
                "--lease-holder",
                "scheduler",
                "--lease-id",
                "lease-1",
                "--zip",
                input.to_str().unwrap(),
                "--out",
                out.to_str().unwrap(),
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
    fn session_request_operation_run_without_daemon_is_runtime_error() {
        let temp = TempDir::new().unwrap();
        let operation_dir = temp.path().join("operation");
        let result = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "request",
                "operation-run",
                "--state-dir",
                temp.path().to_str().unwrap(),
                "--lease-holder",
                "scheduler",
                "--lease-id",
                "lease-1",
                "--operation-dir",
                operation_dir.to_str().unwrap(),
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
    fn session_request_stream_without_daemon_is_runtime_error() {
        let temp = TempDir::new().unwrap();
        let result = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "request",
                "stream",
                "--state-dir",
                temp.path().to_str().unwrap(),
                "--max-frames",
                "1",
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
    fn session_request_capabilities_without_daemon_is_runtime_error() {
        let temp = TempDir::new().unwrap();
        let result = run_cli(
            [
                "--json",
                "session",
                "request",
                "capabilities",
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
    fn session_request_contract_without_daemon_is_runtime_error() {
        let temp = TempDir::new().unwrap();
        let result = run_cli(
            [
                "--json",
                "session",
                "request",
                "contract",
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
    fn session_request_api_without_daemon_is_runtime_error() {
        let temp = TempDir::new().unwrap();
        let result = run_cli(
            [
                "--json",
                "session",
                "request",
                "api",
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
    fn session_request_transport_without_daemon_is_runtime_error() {
        let temp = TempDir::new().unwrap();
        let result = run_cli(
            [
                "--json",
                "session",
                "request",
                "transport",
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
    fn session_request_events_without_daemon_is_runtime_error() {
        let temp = TempDir::new().unwrap();
        let result = run_cli(
            [
                "--json",
                "session",
                "request",
                "events",
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
            "--local".to_string(),
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
    fn session_state_request_payload_preserves_holder_and_lease_id() {
        let args = [
            "acquire".to_string(),
            "--via-daemon".to_string(),
            "--state-dir".to_string(),
            "target/session".to_string(),
            "--request-timeout-ms".to_string(),
            "15000".to_string(),
            "--holder".to_string(),
            "scheduler".to_string(),
            "--lease-id".to_string(),
            "lease-1".to_string(),
        ];

        assert_eq!(
            session_state_request_payload_args(&args),
            vec![
                "acquire".to_string(),
                "--holder".to_string(),
                "scheduler".to_string(),
                "--lease-id".to_string(),
                "lease-1".to_string()
            ]
        );
    }

    #[test]
    fn session_request_journal_records_success_and_error() {
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        unsafe {
            env::set_var(CONFIG_ENV, &config);
        }
        let success = SessionCommandRequest {
            request_id: "request-1".to_string(),
            command: "stream".to_string(),
            global: SessionCommandGlobal {
                instance: Some("ak".to_string()),
                game: None,
                server: None,
                resource_root: None,
                capture_backend: None,
                dry_run: false,
            },
            args: vec![
                "--dry-run".to_string(),
                "--max-frames".to_string(),
                "1".to_string(),
            ],
            lease: None,
            created_at_unix_ms: 1,
        };
        write_json_file_atomic(
            &session_requests_dir(&state_dir).join("request-1.json"),
            &success,
        )
        .unwrap();
        assert_eq!(process_session_requests(&state_dir).unwrap(), 1);

        let failed = SessionCommandRequest {
            request_id: "request-2".to_string(),
            command: "tap".to_string(),
            global: success.global.clone(),
            args: vec!["10".to_string(), "20".to_string()],
            lease: None,
            created_at_unix_ms: 2,
        };
        write_json_file_atomic(
            &session_requests_dir(&state_dir).join("request-2.json"),
            &failed,
        )
        .unwrap();
        assert_eq!(process_session_requests(&state_dir).unwrap(), 1);

        let entries = read_session_request_journal(&state_dir, 10).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].request_id, "request-1");
        assert!(entries[0].ok);
        assert_eq!(entries[1].request_id, "request-2");
        assert!(!entries[1].ok);
        assert_eq!(
            entries[1].error.as_ref().unwrap().code,
            "lab_lease_required"
        );

        let recent = run_cli(
            [
                "--json",
                "session",
                "journal",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--limit",
                "1",
            ],
            true,
        );
        let evented = run_cli(
            [
                "--json",
                "session",
                "events",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--limit",
                "2",
            ],
            true,
        );
        unsafe {
            env::remove_var(CONFIG_ENV);
        }

        assert_eq!(recent.exit_code(), 0);
        let entries = recent
            .envelope
            .data
            .as_ref()
            .unwrap()
            .get("entries")
            .and_then(Value::as_array)
            .unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].get("request_id").and_then(Value::as_str),
            Some("request-2")
        );

        assert_eq!(evented.exit_code(), 0);
        let events = evented
            .envelope
            .data
            .as_ref()
            .unwrap()
            .get("events")
            .and_then(Value::as_array)
            .unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(
            events[1].get("type").and_then(Value::as_str),
            Some("session.request.failed")
        );
        assert_eq!(
            events[1].pointer("/error/code").and_then(Value::as_str),
            Some("lab_lease_required")
        );
    }

    #[test]
    fn session_recover_stale_capture_daemon_request_does_not_require_lease() {
        let temp = TempDir::new().unwrap();
        let state_dir = temp.path().join("session");
        let request = SessionCommandRequest {
            request_id: "request-stale-capture".to_string(),
            command: "recover".to_string(),
            global: SessionCommandGlobal {
                instance: Some("ak".to_string()),
                game: None,
                server: None,
                resource_root: None,
                capture_backend: Some("adb".to_string()),
                dry_run: false,
            },
            args: vec!["--stale-capture".to_string()],
            lease: None,
            created_at_unix_ms: 1,
        };
        write_json_file_atomic(
            &session_requests_dir(&state_dir).join("request-stale-capture.json"),
            &request,
        )
        .unwrap();

        assert_eq!(process_session_requests(&state_dir).unwrap(), 1);
        let response = read_json_file::<SessionCommandResponse>(
            &session_responses_dir(&state_dir).join("request-stale-capture.json"),
        )
        .unwrap()
        .unwrap();

        assert!(response.ok);
        assert_eq!(
            response
                .data
                .as_ref()
                .and_then(|data| data.get("mode"))
                .and_then(Value::as_str),
            Some("stale_capture_recovery")
        );
    }

    #[test]
    fn session_status_diagnostics_reports_queue_and_journal_summary() {
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        unsafe {
            env::set_var(CONFIG_ENV, &config);
        }
        let completed = SessionCommandRequest {
            request_id: "completed-1".to_string(),
            command: "stream".to_string(),
            global: SessionCommandGlobal {
                instance: Some("ak".to_string()),
                game: None,
                server: None,
                resource_root: None,
                capture_backend: None,
                dry_run: false,
            },
            args: vec![
                "--dry-run".to_string(),
                "--max-frames".to_string(),
                "1".to_string(),
            ],
            lease: None,
            created_at_unix_ms: 1,
        };
        write_json_file_atomic(
            &session_requests_dir(&state_dir).join("completed-1.json"),
            &completed,
        )
        .unwrap();
        assert_eq!(process_session_requests(&state_dir).unwrap(), 1);
        write_json_file_atomic(
            &session_requests_dir(&state_dir).join("pending-1.json"),
            &completed,
        )
        .unwrap();
        let response = SessionCommandResponse {
            request_id: "response-1".to_string(),
            command: "stream".to_string(),
            ok: true,
            data: Some(json!({"status": "done"})),
            error: None,
            started_at_unix_ms: 2,
            completed_at_unix_ms: 3,
        };
        write_json_file_atomic(
            &session_responses_dir(&state_dir).join("response-1.json"),
            &response,
        )
        .unwrap();

        let status = run_cli(
            [
                "--json",
                "session",
                "status",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--diagnostics",
            ],
            true,
        );
        unsafe {
            env::remove_var(CONFIG_ENV);
        }

        assert_eq!(status.exit_code(), 0);
        let diagnostics = status
            .envelope
            .data
            .as_ref()
            .unwrap()
            .get("diagnostics")
            .unwrap();
        assert_eq!(
            diagnostics
                .pointer("/queues/pending_requests")
                .and_then(Value::as_u64),
            Some(1)
        );
        assert_eq!(
            diagnostics
                .pointer("/queues/pending_responses")
                .and_then(Value::as_u64),
            Some(2)
        );
        assert_eq!(
            diagnostics
                .pointer("/journal/total_entries")
                .and_then(Value::as_u64),
            Some(1)
        );
        assert_eq!(
            diagnostics
                .pointer("/journal/last_entry/request_id")
                .and_then(Value::as_str),
            Some("completed-1")
        );
    }

    #[test]
    fn session_status_cli_diagnostics_reports_configured_instances() {
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let config_path = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        unsafe {
            env::set_var(CONFIG_ENV, &config_path);
        }
        let mut config = UserConfig::default();
        config.instances.insert(
            "ak-b".to_string(),
            InstanceConfig {
                serial: Some("127.0.0.1:16416".to_string()),
                game: Some("ark".to_string()),
                server: Some("cn-bilibili".to_string()),
                package: Some("com.hypergryph.arknights.bilibili".to_string()),
                adb_path: Some("C:\\Tools\\adb.exe".to_string()),
                capture_backend: Some("nemu_ipc".to_string()),
            },
        );
        write_user_config(&config).unwrap();

        let status = run_cli(
            [
                "--json",
                "session",
                "status",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--diagnostics",
            ],
            true,
        );
        unsafe {
            env::remove_var(CONFIG_ENV);
        }

        assert_eq!(status.exit_code(), 0);
        let diagnostics = status
            .envelope
            .data
            .as_ref()
            .unwrap()
            .get("diagnostics")
            .unwrap();
        assert_eq!(
            diagnostics
                .pointer("/instances/available")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            diagnostics
                .pointer("/instances/count")
                .and_then(Value::as_u64),
            Some(1)
        );
        assert_eq!(
            diagnostics
                .pointer("/instances/instances/0/id")
                .and_then(Value::as_str),
            Some("ak-b")
        );
        assert_eq!(
            diagnostics
                .pointer("/instances/instances/0/server")
                .and_then(Value::as_str),
            Some("cn-bilibili")
        );
        assert_eq!(
            diagnostics
                .pointer("/instances/instances/0/capture_backend")
                .and_then(Value::as_str),
            Some("nemu_ipc")
        );
    }

    #[test]
    fn session_request_journal_rotates_when_active_file_exceeds_retention_limit() {
        let temp = TempDir::new().unwrap();
        let state_dir = temp.path();
        let old_entry = SessionRequestJournalEntry {
            request_id: "old-entry".to_string(),
            command: "stream".to_string(),
            args: vec!["x".repeat(2048)],
            lease: None,
            ok: true,
            error: None,
            created_at_unix_ms: 1,
            started_at_unix_ms: 2,
            completed_at_unix_ms: 3,
        };
        let old_line = format!("{}\n", serde_json::to_string(&old_entry).unwrap());
        let repetitions = (SESSION_REQUEST_JOURNAL_MAX_BYTES as usize / old_line.len()) + 2;
        fs::write(
            session_request_journal_path(state_dir),
            old_line.repeat(repetitions),
        )
        .unwrap();

        let request = SessionCommandRequest {
            request_id: "new-request".to_string(),
            command: "stream".to_string(),
            global: SessionCommandGlobal {
                instance: Some("ak".to_string()),
                game: None,
                server: None,
                resource_root: None,
                capture_backend: None,
                dry_run: false,
            },
            args: vec![
                "--dry-run".to_string(),
                "--max-frames".to_string(),
                "1".to_string(),
            ],
            lease: None,
            created_at_unix_ms: 4,
        };
        let response = SessionCommandResponse {
            request_id: "new-request".to_string(),
            command: "stream".to_string(),
            ok: true,
            data: Some(json!({"status": "done"})),
            error: None,
            started_at_unix_ms: 5,
            completed_at_unix_ms: 6,
        };

        append_session_request_journal(state_dir, &request, &response).unwrap();

        assert!(session_request_journal_archive_path(state_dir).exists());
        assert!(file_size_if_exists(&session_request_journal_archive_path(state_dir)).unwrap() > 0);
        let entries = read_session_request_journal(state_dir, 10).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].request_id, "new-request");

        let status = run_cli(
            [
                "--json",
                "session",
                "status",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--diagnostics",
            ],
            true,
        );
        assert_eq!(status.exit_code(), 0);
        let diagnostics = status
            .envelope
            .data
            .as_ref()
            .unwrap()
            .get("diagnostics")
            .unwrap();
        assert_eq!(
            diagnostics
                .pointer("/journal/total_entries")
                .and_then(Value::as_u64),
            Some(1)
        );
        assert_eq!(
            diagnostics
                .pointer("/journal/retention/max_bytes")
                .and_then(Value::as_u64),
            Some(SESSION_REQUEST_JOURNAL_MAX_BYTES)
        );
        assert_eq!(
            diagnostics
                .pointer("/journal/archive/exists")
                .and_then(Value::as_bool),
            Some(true)
        );
    }

    #[test]
    fn session_journal_corrupt_line_is_runtime_error() {
        let temp = TempDir::new().unwrap();
        fs::write(session_request_journal_path(temp.path()), "not-json\n").unwrap();
        let result = run_cli(
            [
                "--json",
                "session",
                "journal",
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
    fn session_status_diagnostics_corrupt_journal_is_runtime_error() {
        let temp = TempDir::new().unwrap();
        fs::write(session_request_journal_path(temp.path()), "not-json\n").unwrap();
        let result = run_cli(
            [
                "--json",
                "session",
                "status",
                "--state-dir",
                temp.path().to_str().unwrap(),
                "--diagnostics",
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
        let _ = run_cli(
            [
                "--json",
                "config",
                "set",
                "instance.azur.adb_path",
                "C:\\Tools\\adb.exe",
            ],
            true,
        );
        let _ = run_cli(
            [
                "--json",
                "config",
                "set",
                "instance.azur.capture_backend",
                "droidcast_raw",
            ],
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
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let config_path = temp.path().join("config.json");
        unsafe {
            env::set_var(CONFIG_ENV, &config_path);
        }
        let mut config = UserConfig::default();
        config.instances.insert(
            "ak-b".to_string(),
            InstanceConfig {
                serial: Some("127.0.0.1:16416".to_string()),
                game: Some("ark".to_string()),
                server: Some("cn-bilibili".to_string()),
                package: Some("com.hypergryph.arknights.bilibili".to_string()),
                adb_path: Some("C:\\Tools\\adb.exe".to_string()),
                capture_backend: Some("nemu_ipc".to_string()),
            },
        );
        config.instances.insert(
            "ba-jp".to_string(),
            InstanceConfig {
                serial: Some("127.0.0.1:16384".to_string()),
                game: Some("ba".to_string()),
                server: None,
                package: None,
                adb_path: None,
                capture_backend: None,
            },
        );
        write_user_config(&config).unwrap();

        let result = run_cli(["--json", "session", "instance", "registry"], true);
        unsafe {
            env::remove_var(CONFIG_ENV);
        }

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
        let ak = instances
            .iter()
            .find(|instance| instance.get("id").and_then(Value::as_str) == Some("ak-b"))
            .unwrap();
        assert_eq!(
            ak.pointer("/effective/capture_backend")
                .and_then(Value::as_str),
            Some("nemu_ipc")
        );
        assert_eq!(
            ak.pointer("/validation/ready_for_device_control")
                .and_then(Value::as_bool),
            Some(true)
        );
        let ba = instances
            .iter()
            .find(|instance| instance.get("id").and_then(Value::as_str) == Some("ba-jp"))
            .unwrap();
        assert_eq!(
            ba.pointer("/effective/capture_backend")
                .and_then(Value::as_str),
            Some("auto")
        );
        assert_eq!(
            ba.pointer("/validation/ready_for_device_control")
                .and_then(Value::as_bool),
            Some(false)
        );
        assert!(
            ba.pointer("/validation/missing_required_fields")
                .and_then(Value::as_array)
                .unwrap()
                .iter()
                .any(|field| field.as_str() == Some("server"))
        );
    }

    #[test]
    fn session_instance_registry_rejects_invalid_configured_backend() {
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let config_path = temp.path().join("config.json");
        unsafe {
            env::set_var(CONFIG_ENV, &config_path);
        }
        let mut config = UserConfig::default();
        config.instances.insert(
            "ak-b".to_string(),
            InstanceConfig {
                serial: Some("127.0.0.1:16416".to_string()),
                game: Some("ark".to_string()),
                server: Some("cn-bilibili".to_string()),
                package: None,
                adb_path: None,
                capture_backend: Some("not-a-backend".to_string()),
            },
        );
        write_user_config(&config).unwrap();

        let result = run_cli(["--json", "session", "instance", "registry"], true);
        unsafe {
            env::remove_var(CONFIG_ENV);
        }

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
                .contains("invalid instance.ak-b.capture_backend")
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
        assert!(
            data.get("commands")
                .and_then(Value::as_array)
                .unwrap()
                .iter()
                .any(|command| command.get("command").and_then(Value::as_str)
                    == Some("session instance keep-alive"))
        );
    }

    #[test]
    fn session_contract_is_offline_access_contract() {
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
        assert_eq!(
            data.pointer("/daemon_queries/api").and_then(Value::as_str),
            Some("session request api")
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
            data.pointer("/envelopes/event_view/schema_version")
                .and_then(Value::as_str),
            Some("session.events.v0.1")
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
            data.pointer("/envelopes/transport_view/schema_version")
                .and_then(Value::as_str),
            Some("session.transport.v0.1")
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
            data.pointer("/safety/remote_transport_must_not_start_without_authentication")
                .and_then(Value::as_bool),
            Some(true)
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
            Some("available")
        );
    }

    #[test]
    fn lab_lease_capabilities_are_available() {
        let commands = command_capabilities();
        for command_name in [
            "lab status",
            "lab lease",
            "lab lease status",
            "lab preempt",
            "lab release",
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
    fn instance_health_status_reflects_capture_freshness() {
        assert_eq!(instance_health_status(None), "device_connected");
        assert_eq!(
            instance_health_status(Some(CaptureFreshProbeStatus::Fresh)),
            "healthy"
        );
        assert_eq!(
            instance_health_status(Some(CaptureFreshProbeStatus::StaleSuspected)),
            "capture_stale_suspected"
        );
        assert_eq!(
            instance_health_status(Some(CaptureFreshProbeStatus::Unavailable)),
            "capture_unavailable"
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
            data.pointer("/steps/4/type").and_then(Value::as_str),
            Some("app_restart")
        );
        assert_eq!(
            data.pointer("/steps/4/requires_lease")
                .and_then(Value::as_bool),
            Some(true)
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
            "session journal",
            "session events",
            "session contract",
            "session api",
            "session instance",
            "session instance list",
            "session instance health",
            "session instance keep-alive",
            "session instance connect",
            "session instance reconnect",
            "session instance app",
            "session instance app launch",
            "session instance app stop",
            "session instance app restart",
            "session app",
            "session app launch",
            "session app stop",
            "session app restart",
            "session capture",
            "session capture diagnose",
            "session request status",
            "session request journal",
            "session request events",
            "session request contract",
            "session request api",
            "session request devices",
            "session request lease",
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
            "session request instance health",
            "session request instance keep-alive",
            "session request instance connect",
            "session request instance reconnect",
            "session request instance app",
            "session request app",
            "session request lab-run",
            "session request package-run",
            "session request operation-run",
            "session lease",
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
        let _guard = ENV_LOCK.lock().unwrap();
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
        let _guard = ENV_LOCK.lock().unwrap();
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
    fn device_config_uses_instance_capture_backend_default() {
        let global = GlobalOptions {
            instance: Some("ak-b".to_string()),
            ..Default::default()
        };
        let mut config = UserConfig::default();
        config.instances.insert(
            "ak-b".to_string(),
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
            instance: Some("ak-b".to_string()),
            capture_backend: Some(CaptureBackendChoice::Adb),
            ..Default::default()
        };
        let mut config = UserConfig::default();
        config.instances.insert(
            "ak-b".to_string(),
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
