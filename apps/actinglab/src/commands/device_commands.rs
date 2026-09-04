use crate::{
    CliError, CliOutcome, DeviceRuntimeConfig, FlagArgs, GlobalOptions, device_config,
    parse_optional_duration_ms, parse_touch_backend_override, read_user_config,
    resolve_instance_id, runtime_capture_backend, runtime_input_backend, runtime_state_root,
    stream_input_relay_action,
};
use actingcommand_contract::{EventActor, EventSource};
use actingcommand_device::{
    CaptureBackendChoice, Frame, InputBackend, combine_operation_and_close,
};
use actingcommand_lab::UserConfig;
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::time::Duration;

pub(crate) fn run_touch_probe(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
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

pub(crate) fn run_capture(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
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

pub(crate) fn reject_legacy_session_routing(flags: &FlagArgs) -> CliOutcome<()> {
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

pub(crate) struct CaptureCommandResult {
    pub(crate) frame: Frame,
    pub(crate) attempts: Vec<Value>,
    pub(crate) freshness: Value,
}

pub(crate) struct CaptureFreshProbeReport {
    pub(crate) status: CaptureFreshProbeStatus,
    pub(crate) frame: Option<Frame>,
    pub(crate) attempts: Vec<Value>,
    pub(crate) freshness: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CaptureFreshProbeStatus {
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
pub(crate) enum CaptureFreshnessExpectation {
    StaticPageAllowed,
    ExpectedChange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CaptureFreshnessDecision {
    pub(crate) status: CaptureFreshProbeStatus,
    pub(crate) ok: bool,
    pub(crate) stale_suspected: bool,
    reason: &'static str,
}

pub(crate) fn capture_for_command(
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

pub(crate) fn capture_fresh_probe_report(
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

pub(crate) fn classify_capture_freshness(
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

pub(crate) fn capture_fresh_probe_report_json(
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
pub(crate) fn instance_health_status(
    capture_status: Option<CaptureFreshProbeStatus>,
) -> &'static str {
    match capture_status {
        Some(CaptureFreshProbeStatus::Fresh) => "healthy",
        Some(CaptureFreshProbeStatus::StaticUnchanged) => "healthy_static",
        Some(CaptureFreshProbeStatus::StaleSuspected) => "capture_stale_suspected",
        None => "device_connected",
    }
}

pub(crate) fn capture_diagnosis_recovery_json(
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DirectTouchCommand {
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
    pub(crate) fn parse(command: &str, flags: &FlagArgs) -> CliOutcome<Self> {
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

pub(crate) fn open_cli_runtime_input_proxy(
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

pub(crate) fn run_direct_touch(
    global: &GlobalOptions,
    command: &str,
    args: &[String],
) -> CliOutcome<Value> {
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

pub(crate) fn run_direct_input(
    global: &GlobalOptions,
    command: &str,
    args: &[String],
) -> CliOutcome<Value> {
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
pub(crate) enum DirectInputCommand {
    Key(String),
    Text(String),
}

impl DirectInputCommand {
    pub(crate) fn parse(command: &str, flags: &FlagArgs) -> CliOutcome<Self> {
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
pub(crate) enum StreamInputRelayAction {
    Touch(DirectTouchCommand),
    Input(DirectInputCommand),
}

impl StreamInputRelayAction {
    pub(crate) fn parse_many(flags: &FlagArgs) -> CliOutcome<Vec<Self>> {
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

    pub(crate) fn to_json(&self) -> Value {
        match self {
            Self::Touch(command) => command.to_json(),
            Self::Input(command) => command.to_json(),
        }
    }
}

pub(crate) fn run_stream_input_relay(
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
