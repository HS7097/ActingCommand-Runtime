// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_device::{
    Adb, AdbConfig, CaptureBackendChoice, CaptureBackendConfig, DeviceTarget,
    create_capture_backend,
};
use actingcommand_page_detector::{PageDetector, load_page_set_from_json_str};
use actingcommand_recognition::{MatchMetric, Scene};
use actingcommand_recognition_pack::{RecognitionEvaluator, load_pack_from_json_str};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs::{self, File};
use std::io::{self, IsTerminal, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;
use zip::write::FileOptions;
use zip::{ZipArchive, ZipWriter};

mod lab_run;

const SCHEMA_VERSION: &str = "0.2";
const RUNTIME_VERSION: &str = "runtime-embedded-p1g";
const DEFAULT_ADB_HINT: &str = r"F:\AzurPilot\.venv\Scripts\adb.exe";
const CONFIG_ENV: &str = "ACTINGLAB_CONFIG_PATH";
const DANGEROUS_EXTENSIONS: &[&str] = &[
    "py", "exe", "bat", "cmd", "ps1", "sh", "js", "vbs", "msi", "dll", "scr", "com", "jar",
];

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

#[derive(Debug, Serialize)]
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
        | "run" | "report" => rest.get(1).map(|_| 2).unwrap_or(1),
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
        [cmd] if cmd == "capture" => run_capture(&invocation.global, &invocation.args),
        [cmd] if cmd == "detect-page" => run_detect_page(&invocation.global, &invocation.args),
        [cmd] if cmd == "recognize" => run_recognize(&invocation.global, &invocation.args),
        [cmd] if cmd == "monitor" => run_monitor(&invocation.global),
        [cmd] if cmd == "record" => Err(CliError::not_implemented(
            "not_implemented",
            "record is reserved for Runtime frame-stream integration",
        )),
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
        [group, sub] if group == "resource" => run_resource(sub, &invocation.args),
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
    Ok(json!({
        "config_path": config_path()?.display().to_string(),
        "run_root": global.run_root.as_ref().map(|path| path_string(path)).or(config.run_root),
        "resource_root": global.resource_root.as_ref().map(|path| path_string(path)).or(config.resource_root),
        "runtime_endpoint": global.runtime_endpoint.clone().or(config.runtime_endpoint),
        "default_adb_hint": DEFAULT_ADB_HINT
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
    let adb_path = effective_adb_path(global, &config);
    let runtime_endpoint = effective_runtime_endpoint(global, &config);
    let resource_root = effective_resource_root(global, &config);
    let run_root = effective_run_root(global, &config);
    let mut checks = Vec::new();

    checks.push(json!({
        "name": "config",
        "ok": config_path()?.exists(),
        "path": config_path()?.display().to_string()
    }));
    checks.push(json!({
        "name": "adb",
        "ok": Path::new(&adb_path).is_file() || adb_path == "adb",
        "path": adb_path,
        "hint": DEFAULT_ADB_HINT
    }));
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

fn run_devices(global: &GlobalOptions) -> CliOutcome<Value> {
    let config = read_user_config()?;
    let adb_path = effective_adb_path(global, &config);
    let adb = Adb::new(AdbConfig {
        adb_path,
        ..Default::default()
    });
    let output = adb
        .run(&["devices", "-l"])
        .map_err(|err| CliError::device(err.to_string()))?;
    Ok(json!({
        "adb_stdout": output.stdout,
        "adb_stderr": output.stderr
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
            "rules": [
                "CLI capture backend overrides control capture_backend",
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
    let out = flags.required_path("--out")?;
    let config = read_user_config()?;
    let device_config = device_config(global, &config)?;
    let requested = global.capture_backend.unwrap_or_default();
    let selected = create_capture_backend(device_config.capture_backend_config(requested))
        .map_err(|err| CliError::device(err.to_string()))?;
    let mut backend = selected.backend;
    let frame = backend
        .capture()
        .map_err(|err| CliError::device(err.to_string()))?;
    if let Some(parent) = out.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(|err| {
            CliError::device(format!("failed to create {}: {err}", parent.display()))
        })?;
    }
    fs::write(&out, &frame.png)
        .map_err(|err| CliError::device(format!("failed to write {}: {err}", out.display())))?;
    Ok(json!({
        "width": frame.width,
        "height": frame.height,
        "capture_backend_used": frame.backend_name.as_str(),
        "capture_backend_attempts": selected.diagnostics.attempts.iter().map(|attempt| json!({
            "backend": attempt.backend.as_str(),
            "ok": attempt.ok,
            "message": attempt.message
        })).collect::<Vec<_>>(),
        "out": out.display().to_string()
    }))
}

fn run_recognize(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
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
    let evaluations = detector
        .evaluate_all(&evaluator, &scene)
        .map_err(|err| CliError::usage(err.to_string()))?;
    if let Some(match_eval) = evaluations.iter().find(|evaluation| evaluation.matched) {
        return Ok(json!({
            "page": match_eval.page_id,
            "matched": true,
            "standby": false,
            "evaluations": evaluations.iter().map(page_eval_json).collect::<Vec<_>>()
        }));
    }
    Ok(json!({
        "page": "standby",
        "matched": false,
        "standby": true,
        "recovery_hint": {
            "action": "wake_safe_point",
            "point": {"x": 300, "y": 2},
            "note": "CLI does not click automatically"
        },
        "evaluations": evaluations.iter().map(page_eval_json).collect::<Vec<_>>()
    }))
}

fn run_monitor(global: &GlobalOptions) -> CliOutcome<Value> {
    require_runtime(global)?;
    Ok(json!({
        "mode": "passive_mirror",
        "click_allowed": false,
        "scheduler_pause": false,
        "status": "reserved"
    }))
}

fn run_lab(sub: &str, global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    match sub {
        "run" => lab_run::run_lab_run(global, args),
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

fn run_resource(sub: &str, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    let repo = flags.required_path("--repo")?;
    match sub {
        "validate" => validate_resource_repo(&repo),
        "convert" => Ok(json!({
            "repo": repo.display().to_string(),
            "status": "reserved",
            "note": "resource convert is an offline data command; repository-specific converters run outside Runtime"
        })),
        "import-alas" | "drift-alas" => {
            let alas_root = flags.required_path("--alas-root")?;
            Ok(json!({
                "repo": repo.display().to_string(),
                "alas_root": alas_root.display().to_string(),
                "status": "reserved",
                "command": sub
            }))
        }
        "check-release" => Ok(json!({
            "repo": repo.display().to_string(),
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
    let instance_id = resolve_instance_id(global, config)?;
    let instance = config.instances.get(&instance_id);
    let mut target = DeviceTarget::default();
    if let Some(serial) = instance.and_then(|instance| instance.serial.clone()) {
        target.serial = Some(serial);
    } else if global.instance.as_deref() == Some(instance_id.as_str()) && instance.is_none() {
        target.serial = Some(instance_id.clone());
    }
    let adb = AdbConfig {
        adb_path: effective_adb_path(global, config),
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
    let root = env::var("LOCALAPPDATA")
        .or_else(|_| env::var("APPDATA"))
        .map_err(|_| CliError::usage("LOCALAPPDATA or APPDATA is required for config path"))?;
    Ok(PathBuf::from(root)
        .join("ActingCommand")
        .join("actinglab")
        .join("config.json"))
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
            "instance config keys use instance.<id>.serial|game|server",
        ));
    }
    let instance = config.instances.get(parts[1]);
    let value = match parts[2] {
        "serial" => instance.and_then(|instance| instance.serial.clone()),
        "game" => instance.and_then(|instance| instance.game.clone()),
        "server" => instance.and_then(|instance| instance.server.clone()),
        other => return Err(CliError::usage(format!("unknown instance field: {other}"))),
    };
    Ok(json!(value))
}

fn set_instance_value(config: &mut UserConfig, key: &str, value: &str) -> CliOutcome<()> {
    let parts = key.split('.').collect::<Vec<_>>();
    if parts.len() != 3 {
        return Err(CliError::usage(
            "instance config keys use instance.<id>.serial|game|server",
        ));
    }
    let instance = config.instances.entry(parts[1].to_string()).or_default();
    match parts[2] {
        "serial" => instance.serial = Some(value.to_string()),
        "game" => instance.game = Some(value.to_string()),
        "server" => instance.server = Some(value.to_string()),
        other => return Err(CliError::usage(format!("unknown instance field: {other}"))),
    }
    Ok(())
}

fn effective_adb_path(_global: &GlobalOptions, config: &UserConfig) -> String {
    config
        .adb_path
        .clone()
        .unwrap_or_else(|| DEFAULT_ADB_HINT.to_string())
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
}

fn effective_run_root(global: &GlobalOptions, config: &UserConfig) -> Option<PathBuf> {
    global
        .run_root
        .clone()
        .or_else(|| config.run_root.as_ref().map(PathBuf::from))
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
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).map_err(|err| {
            CliError::package_invalid(format!("failed to read zip entry {path_name}: {err}"))
        })?;
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
    for (path, expected) in manifest_hashes(manifest) {
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

fn manifest_hashes(manifest: &Value) -> Vec<(String, String)> {
    let mut hashes = Vec::new();
    if let Some(object) = manifest.get("hashes").and_then(Value::as_object) {
        for (path, value) in object {
            if let Some(hash) = value.as_str() {
                hashes.push((path.clone(), hash.to_string()));
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
                hashes.push((path.to_string(), hash.to_string()));
            }
        }
    }
    hashes
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
    let runs = if run_root.is_dir() {
        fs::read_dir(run_root)
            .map_err(|err| {
                CliError::usage(format!("failed to list {}: {err}", run_root.display()))
            })?
            .filter_map(Result::ok)
            .filter(|entry| entry.path().is_dir())
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    Ok(json!({
        "run_root": run_root.display().to_string(),
        "runs": runs
    }))
}

fn list_resource_kind(root: &Path, kind: &str) -> CliOutcome<Value> {
    let suffix = match kind {
        "targets" => ".pack.json",
        "pages" => ".pages.json",
        "tasks" | "bundles" => "task.json",
        "controls" => ".controls.json",
        _ => unreachable!("validated list kind"),
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
        let selected = create_capture_backend(device_config.capture_backend_config(requested))
            .map_err(|err| CliError::device(err.to_string()))?;
        let mut backend = selected.backend;
        let frame = backend
            .capture()
            .map_err(|err| CliError::device(err.to_string()))?;
        return Scene::from_png(&frame.png).map_err(|err| CliError::device(err.to_string()));
    }
    Err(CliError::usage(
        "command requires --scene <png> or --capture",
    ))
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
        command_cap("resource convert", ["offline"], "reserved"),
        command_cap("resource import-alas", ["offline"], "reserved"),
        command_cap("resource drift-alas", ["offline"], "reserved"),
        command_cap("resource check-release", ["offline"], "available"),
        command_cap("package validate", ["offline"], "available"),
        command_cap("package inspect", ["offline"], "available"),
        command_cap("operation validate", ["offline"], "available"),
        command_cap("operation inspect", ["offline"], "available"),
        command_cap("operation explain", ["offline"], "available"),
        command_cap("status", ["running_runtime"], "available"),
        command_cap("devices", ["device"], "available"),
        command_cap("monitor", ["running_runtime"], "reserved"),
        command_cap("scheduler status", ["running_runtime"], "reserved"),
        command_cap("scheduler pause", ["running_runtime"], "reserved"),
        command_cap("scheduler resume", ["running_runtime"], "reserved"),
        command_cap("scheduler start", ["running_runtime"], "reserved"),
        command_cap("scheduler stop", ["running_runtime"], "reserved"),
        command_cap("lab status", ["running_runtime"], "reserved"),
        command_cap("lab lease", ["running_runtime"], "reserved"),
        command_cap("lab release", ["running_runtime"], "reserved"),
        command_cap("lab run", ["device"], "available"),
        command_cap("capture", ["device"], "available"),
        command_cap("detect-page", ["device"], "available"),
        command_cap("recognize", ["device"], "available"),
        command_cap("record", ["running_runtime", "device"], "reserved"),
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
