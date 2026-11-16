use crate::flag_args::reads::{FlagReads, ReadPart};
use crate::{
    CliError, CliOutcome, DeviceRuntimeConfig, FlagArgs, GlobalOptions, device_config,
    parse_optional_duration_ms, parse_touch_backend_override, read_user_config,
    resolve_instance_id, runtime_capture_backend, runtime_input_backend, runtime_state_root,
    stream_input_relay_action,
};
use actingcommand_contract::{
    EmulatorCapability, InputAction, InputFrameReference, RuntimeInstanceStatus, RuntimeReceipt,
    RuntimeReceiptState, RuntimeResult,
};
use actingcommand_device::{
    CaptureBackendChoice, Frame, NemuInputSupport, TouchBackendChoice, combine_operation_and_close,
};
use actingcommand_lab::{
    AfterCapture, AttachFrameRequest, CapturedFrame, ClickEffect, ClickPlan, CommitClickRequest,
    LAB_RECORDING_MAX_NEXT_FRAME_MS, LabInputPort, OpaqueJson, PlanClickRequest, PlannedSwipe,
    PointPress, RecordFramePorts, RecordPoint, RecordPressRequest, UserConfig, record_attach_frame,
    record_frame_preflight, record_press, record_press_frame_unavailable, record_press_input,
};
use actingcommand_runtime_client::{
    RuntimeClient, RuntimeClientError, RuntimeClientErrorClass, RuntimeInputProxy,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::time::{Duration, Instant};

/// Workflow #434 LAB-5B3: the flags `touch-probe` reads: `--touch-backend`, only to refuse it
/// (actingd owns the backend; actinglab's parser takes the flag globally). Workflow #434 LAB-5B4:
/// its command line is parsed by it (`FlagArgs::parse_reading`); no positional.
pub(crate) const TOUCH_PROBE_READS: FlagReads = FlagReads {
    row: "touch-probe",
    parts: &[&ReadPart {
        options: &[],
        switches: &[],
        refused: &["--touch-backend"],
    }],
    positionals: &[],
};

pub(crate) fn run_touch_probe(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse_reading(args, &TOUCH_PROBE_READS)?;
    flags.expect_positionals("touch-probe", 0)?;
    if parse_touch_backend_override(&flags)?.is_some() || global.touch_backend.is_some() {
        return Err(CliError::usage(
            "touch-probe backend selection is owned by actingd; remove --touch-backend",
        ));
    }
    crate::dry_run_gate::refuse(
        global,
        "touch-probe",
        Some("session status --diagnostics"),
        Value::Null,
    )?;
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

/// Workflow #434 LAB-5B4: the flags `capture` (and `session capture`) reads, on its two paths: the
/// legacy routing it refuses, `diagnose` as its positional or `--diagnose`, the frame's freshness,
/// and, to capture, `--out` and `--record` with the recording's instance (`--instance` is
/// actinglab's global option, which its parser takes before the command sees its flags).
pub(crate) const CAPTURE_READS: FlagReads = FlagReads {
    row: "capture",
    parts: &[
        &LEGACY_ROUTING_READS,
        &ReadPart {
            options: &["--fresh-delay-ms"],
            switches: &["--diagnose", "--require-fresh"],
            refused: &[],
        },
        &ReadPart {
            options: &["--out", "--instance"],
            switches: &["--record"],
            refused: &[],
        },
    ],
    positionals: &["diagnose"],
};

pub(crate) fn run_capture(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse_reading(args, &CAPTURE_READS)?;
    if flags.bool("--diagnose")
        || flags
            .positionals
            .first()
            .is_some_and(|value| value == "diagnose")
    {
        return run_capture_diagnose(global, &flags);
    }
    crate::dry_run_gate::refuse(
        global,
        "capture --out",
        Some("capture diagnose"),
        Value::Null,
    )?;
    reject_legacy_session_routing(&flags)?;
    let record = flags.bool("--record");
    let out = if record {
        flags.optional_path("--out")
    } else {
        Some(flags.required_path("--out")?)
    };
    let config = read_user_config()?;
    // `capture --record`: the recording must accept a device frame before anything is captured.
    let record_lock = if record {
        let instance = resolve_instance_id(global, &config)?;
        let lock = super::session_record::record_flag_begin(
            global,
            &flags,
            &instance,
            "capture --record",
        )?;
        record_frame_preflight(&lock)?;
        Some(lock)
    } else {
        None
    };
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
    if let Some(parent) = out.as_ref().and_then(|out| out.parent())
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(|err| {
            CliError::device(format!("failed to create {}: {err}", parent.display()))
        })?;
    }
    let png = frame
        .png_for_artifact()
        .map_err(|err| CliError::device(err.to_string()))?;
    if let Some(out) = &out {
        fs::write(out, &png)
            .map_err(|err| CliError::device(format!("failed to write {}: {err}", out.display())))?;
    }
    let mut data = json!({
        "width": frame.width,
        "height": frame.height,
        "capture_backend_used": frame.backend_name.as_str(),
        "adb_source": device_config.adb_source.as_str(),
        "adb_warning": device_config.adb_warning,
        "capture_backend_attempts": captured.attempts,
        "freshness": captured.freshness,
        "out": out.as_ref().map(|out| out.display().to_string())
    });
    if let Some(lock) = &record_lock {
        let attached = record_attach_frame(
            lock,
            AttachFrameRequest {
                png,
                source: "lab_capture".to_string(),
                runtime_artifact: None,
                capture_backend: Some(frame.backend_name.as_str().to_string()),
                freshness: Some(OpaqueJson::from_serializable(&captured.freshness)?),
            },
        )?;
        data["record"] = serde_json::to_value(attached)
            .map_err(|error| CliError::usage(format!("failed to encode the record: {error}")))?;
    }
    Ok(data)
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

/// Workflow #434 LAB-5B3: the flags [`reject_legacy_session_routing`] reads, each only to refuse
/// it: a part of the read lists of the own commands that call it.
pub(crate) const LEGACY_ROUTING_READS: ReadPart = ReadPart {
    options: &[],
    switches: &[],
    refused: &["--via-daemon", "--local", "--state-dir"],
};

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
    /// #463 S1-1 fix round 2: the reference of the frame the Runtime committed for the one
    /// observation (`runtime_capture_backend::input_frame_reference`), which an input planned on
    /// the frame names; `None` for a fresh capture's sequence, on which nothing is pressed.
    pub(crate) input_frame: Option<InputFrameReference>,
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

    capture_observation_for_command(&device_config.runtime_capture_endpoint())
}

/// #463 S1-1 fix round 3: [`capture_for_command`] without `--require-fresh` on the Runtime's
/// capture endpoint of an instance: the Runtime captures, so no local device configuration (no
/// ADB path) is read here, and `tap`, `swipe` and `long-tap --record` take their frames so.
fn capture_observation_for_command(
    endpoint: &runtime_capture_backend::RuntimeCaptureEndpoint,
) -> CliOutcome<CaptureCommandResult> {
    // #463 S1-1 fix round 2: the one `ObserveReadonly` read it always was, now with the reference
    // of the frame the Runtime committed for it.
    let (frame, input_frame) = runtime_capture_backend::capture_runtime_observation(endpoint)
        .map_err(|err| CliError::device(err.to_string()))?;
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
        input_frame,
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
            input_frame: None,
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

/// Workflow #434 LAB-5B2: the flags `tap`, `swipe` and `long-tap` take beside their positional
/// arguments: #463 S1-1, `--record` (the press is a step of the instance's recording) and its wait
/// before the screen after the press, `--after-ms <ms>`; #463 S1-2, how the screen after it is
/// captured until it is still, `--interval-ms <ms>`, `--still-ms <ms>` and `--still-max-ms <ms>`
/// (the four with `--record` only, `record_flag_gate`).
pub(crate) const DIRECT_TOUCH_FLAGS: [&str; 5] = [
    "--record",
    "--after-ms",
    "--interval-ms",
    "--still-ms",
    "--still-max-ms",
];

/// #463 S1-1: of [`DIRECT_TOUCH_FLAGS`], those that take no value, as the command line of `tap`,
/// `swipe` and `long-tap` is parsed (`FlagArgs::parse_switches`).
pub(crate) const DIRECT_TOUCH_SWITCHES: [&str; 1] = ["--record"];

impl DirectTouchCommand {
    pub(crate) fn parse(command: &str, flags: &FlagArgs) -> CliOutcome<Self> {
        flags.reject_flags(command, &DIRECT_TOUCH_FLAGS)?;
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

    fn run(
        &self,
        backend: &mut runtime_input_backend::RuntimeInputBackend,
    ) -> actingcommand_device::DeviceResult<RuntimeReceipt> {
        backend.input_receipt(self.action())
    }

    /// The Runtime input action this command sends.
    fn action(&self) -> InputAction {
        match *self {
            Self::Tap { x, y } => InputAction::Tap { x, y },
            Self::Swipe {
                x1,
                y1,
                x2,
                y2,
                duration_ms,
            } => InputAction::Swipe {
                x1,
                y1,
                x2,
                y2,
                duration_ms,
            },
            Self::LongTap { x, y, duration_ms } => InputAction::LongTap { x, y, duration_ms },
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

    /// #463 S1-1: the command's name, as its recording lock names it.
    fn name(&self) -> &'static str {
        match self {
            Self::Tap { .. } => "tap",
            Self::Swipe { .. } => "swipe",
            Self::LongTap { .. } => "long-tap",
        }
    }

    /// #463 S1-1: the press `--record` records for this command: a tap or a long press by its
    /// point, a swipe by its two points and duration as `do --capture --record --swipe` plans it;
    /// the first capture after it `after_ms` after the press (100 without it).
    fn record_press_request(&self, after_ms: Option<u64>) -> RecordPressRequest {
        let (click, point_press) = match *self {
            Self::Tap { x, y } => (
                PlanClickRequest::default(),
                Some(PointPress::Tap {
                    point: RecordPoint { x, y },
                }),
            ),
            Self::LongTap { x, y, duration_ms } => (
                PlanClickRequest::default(),
                Some(PointPress::LongPress {
                    point: RecordPoint { x, y },
                    duration_ms,
                }),
            ),
            Self::Swipe {
                x1,
                y1,
                x2,
                y2,
                duration_ms,
            } => (
                PlanClickRequest {
                    swipe: Some(PlannedSwipe {
                        from: RecordPoint { x: x1, y: y1 },
                        to: RecordPoint { x: x2, y: y2 },
                        duration_ms,
                    }),
                    ..PlanClickRequest::default()
                },
                None,
            ),
        };
        RecordPressRequest {
            click,
            point_press,
            after: AfterCapture::for_press(after_ms),
        }
    }
}

pub(crate) fn open_cli_runtime_input_proxy(
    global: &GlobalOptions,
    config: &UserConfig,
) -> CliOutcome<(runtime_input_backend::RuntimeInputBackend, String)> {
    let instance_alias = resolve_instance_id(global, config)?;
    let client = RuntimeClient::connect(crate::lab_client_config(runtime_state_root()?))
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
    let (command, config, flags) = (|| -> CliOutcome<_> {
        let flags = FlagArgs::parse_switches(args, &DIRECT_TOUCH_SWITCHES);
        reject_legacy_session_routing(&flags)?;
        Ok((
            DirectTouchCommand::parse(command, &flags)?,
            read_user_config()?,
            flags,
        ))
    })()
    .map_err(input_not_submitted)?;
    if global.dry_run {
        let instance = resolve_instance_id(global, &config).map_err(input_not_submitted)?;
        return Ok(crate::dry_run_gate::input_preview(
            &instance,
            command.to_json(),
        ));
    }
    // #463 S1-1: `--record` (never with `--dry-run`, which `record_flag_gate` refuses first).
    if flags.bool("--record") {
        return run_recorded_touch(global, &config, &flags, &command);
    }
    send_direct_touch_command(
        global,
        &config,
        &command,
        "direct_trusted_manual",
        "not_required_for_manual_control",
    )
}

/// `tap`, `swipe` or `long-tap` without `--record`: the press through the Runtime's input proxy.
/// Workflow #434 LW-1 (register §1.435): on an instance whose input presses it only on a frame the
/// Runtime captured ([`press_frame`]), one capture first, and the press sent on that frame, as a
/// recorded press sends it (#463 S1-1); the answer then names it (`input_frame`). On every other
/// instance nothing is captured and the press is sent as before.
fn send_direct_touch_command(
    global: &GlobalOptions,
    config: &UserConfig,
    command: &DirectTouchCommand,
    control_mode: &str,
    safety_gate: &str,
) -> CliOutcome<Value> {
    let instance_alias = resolve_instance_id(global, config).map_err(input_not_submitted)?;
    let state_root = runtime_state_root().map_err(input_not_submitted)?;
    let client = RuntimeClient::connect(crate::lab_client_config(&state_root))
        .map_err(|error| input_not_submitted(CliError::device(error.to_string())))?;
    let action = command.action();
    let frame =
        press_frame(&client, &instance_alias, &action, state_root).map_err(input_not_submitted)?;
    let mut backend = runtime_input_backend::RuntimeInputBackend::connect(client, &instance_alias)
        .map_err(|error| input_not_submitted(CliError::device(error.to_string())))?;
    let operation = backend.input_receipt_on(action, frame);
    let close = backend.close();
    let input_outcome = finish_direct_input(operation, close)?;
    let mut answer = direct_touch_answer(
        &instance_alias,
        command,
        control_mode,
        safety_gate,
        input_outcome,
    );
    if let Some(frame) = frame {
        answer["input_frame"] = json!(frame);
    }
    Ok(answer)
}

/// Workflow #434 LW-1 (register §1.435): the frame a plain press of `action` on `instance` is sent
/// on. The Nemu input (the emulator's own interface) performs a tap, a long tap, a swipe and the
/// segmented drags only on a frame the Runtime captured and committed
/// (`NemuInputSupport::InFrame`) and refuses one without (`nemu_input_frame_required`), so for
/// such an action one `Status` read on the press's connection asks which input performs it: the
/// instance's capability table names it (`evidence_ref` of the action's capability, the
/// configured touch backend). When it is the Nemu input, one capture through the Runtime
/// (`ObserveReadonly`, on a connection of its own, as a recorded press captures the frame before
/// it) gives the frame, whose reference the press names; otherwise, and for an instance the status
/// lists without a capability table, `None` and nothing is captured. A status read or a capture
/// that fails is the command's failure, nothing pressed.
fn press_frame(
    client: &RuntimeClient,
    instance: &str,
    action: &InputAction,
    state_root: std::path::PathBuf,
) -> CliOutcome<Option<InputFrameReference>> {
    let Some(capability) = press_capability(action) else {
        return Ok(None);
    };
    if NemuInputSupport::of(action) != NemuInputSupport::InFrame {
        return Ok(None);
    }
    let status = client
        .status()
        .map_err(|error| CliError::device(error.to_string()))?;
    let nemu = status
        .instances()
        .iter()
        .find(|row| row.instance_alias() == instance)
        .and_then(RuntimeInstanceStatus::capabilities)
        .is_some_and(|profile| {
            profile.evidence(capability).evidence_ref() == TouchBackendChoice::NemuIpc.as_str()
        });
    if !nemu {
        return Ok(None);
    }
    let endpoint =
        runtime_capture_backend::RuntimeCaptureEndpoint::new(instance.to_owned(), state_root);
    let (_, frame) = runtime_capture_backend::capture_runtime_observation(&endpoint)
        .map_err(|error| CliError::device(error.to_string()))?;
    frame.map(Some).ok_or_else(|| {
        CliError::device(
            "the Runtime's observation carries no frame reference, which a press on this \
             instance's input names; nothing was pressed",
        )
    })
}

/// Workflow #434 LW-1: the capability of the instance's capability table that performs `action`,
/// for the pointer inputs a plain press sends; `None` for every other action.
fn press_capability(action: &InputAction) -> Option<EmulatorCapability> {
    match action {
        InputAction::Tap { .. } => Some(EmulatorCapability::InputTap),
        InputAction::LongTap { .. } => Some(EmulatorCapability::InputLongTap),
        InputAction::Swipe { .. } => Some(EmulatorCapability::InputSwipe),
        _ => None,
    }
}

/// The answer of a direct touch command that sent its input.
fn direct_touch_answer(
    instance_alias: &str,
    command: &DirectTouchCommand,
    control_mode: &str,
    safety_gate: &str,
    input_outcome: Value,
) -> Value {
    json!({
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
        "action": command.to_json(),
        "input_outcome": input_outcome
    })
}

/// #463 S1-1 (`contracts/lab-recording.md`, "Recording while playing"): `tap`, `swipe` or
/// `long-tap` with `--record`: the press is a step of the instance's recording, with the screen
/// before it and the screen after it. The recording lock is taken as every `--record` command takes
/// it (`record_flag_begin`); fix round 3: no local device configuration is resolved (the Runtime
/// captures and presses, as for `tap` without `--record`), and the recording does the rest under
/// the lock (`record_press`): the press planned on the open step (every refusal before anything is
/// captured or pressed), a frame captured immediately before the press (fix round 2: always), the
/// input sent through the Runtime's input proxy with that frame's reference (built as `do
/// --capture`'s online scene builds one), once more on a new capture when the Runtime no longer
/// held that frame (fix round 3), its outcome recorded, and after a performed press the wait
/// (`--after-ms`, 100 without it) and the frame after it, both captures as `capture --record` takes
/// its frame. The answer is the command's own with `record`; a press that was not performed
/// answers as the command does without `--record` and records no press. A failure after a
/// performed and recorded press (the input lease's release, the receipt's encoding) is answered
/// after the frame after it, with `record` and `input_outcome` in its details. #463 S1-2: after the
/// wait the screen is captured until it is still (`--interval-ms`, `--still-ms`, `--still-max-ms`,
/// `record_after_frame`), every capture kept; the last is the frame after. #463 S1-9 ("The
/// press"): the input proxy, and with it the input lease, is the recorded press's as a whole: taken
/// at its first input, the one retry sent through it, held while the screen after it is captured,
/// and released once `record_press` has answered, whatever it answered; the input outcome's
/// `close_stage` says what the release did (`note_release`, `with_release`).
fn run_recorded_touch(
    global: &GlobalOptions,
    config: &UserConfig,
    flags: &FlagArgs,
    command: &DirectTouchCommand,
) -> CliOutcome<Value> {
    let after_ms = flags
        .optional("--after-ms")
        .map(|value| parse_after_ms(&value))
        .transpose()?;
    let mut request = command.record_press_request(after_ms);
    request.after.interval_ms = after_capture_ms(flags, "--interval-ms", "interval_ms")?;
    request.after.still_ms = after_capture_ms(flags, "--still-ms", "still_ms")?;
    if let Some(bound) = after_capture_ms(flags, "--still-max-ms", "still_max_ms")? {
        request.after.still_max_ms = bound;
    }
    let instance = resolve_instance_id(global, config)?;
    let lock = super::session_record::record_flag_begin(
        global,
        flags,
        &instance,
        &format!("{} --record", command.name()),
    )?;
    // Fix round 3: the Runtime captures and presses, so no local device configuration (no ADB
    // path) is resolved, as `tap` without `--record` resolves none.
    let endpoint = runtime_capture_backend::RuntimeCaptureEndpoint::new(
        instance.clone(),
        runtime_state_root()?,
    );
    let mut frames = PressFrames {
        endpoint: &endpoint,
        origin: Instant::now(),
    };
    let mut sent = None;
    // #463 S1-9: the press's input proxy, none until its first input connects it.
    let mut held = None;
    let mut press = |plan: &ClickPlan,
                     frame: InputFrameReference|
     -> CliOutcome<CommitClickRequest> {
        let pressed = send_recorded_press(&instance, &mut held, record_press_input(plan)?, frame)?;
        let commit = pressed.commit.clone();
        sent = Some(pressed);
        Ok(commit)
    };
    let recorded = record_press(&lock, &request, &mut press, &mut frames);
    // #463 S1-9: the lease is released here, after the last capture after the press (or at once
    // when none was taken), whatever the recording answered.
    let released = held.take().map(|mut proxy| proxy.close());
    // Not sent: the recording's refusal, or the input's own error, as they are.
    let Some(SentPress {
        mut input_outcome,
        mut failures,
        hold_warning,
        ..
    }) = sent
    else {
        let error = recorded.err().unwrap_or_else(|| {
            CliError::device(
                "the recording answered a press the command never sent; nothing more is known",
            )
        });
        return Err(with_release(error, released));
    };
    // A press that was sent was sent through the held proxy, so its release is known; a release
    // that failed is the first failure after the input, as it was when the lease was the press's
    // alone.
    if let Some(released) = &released {
        note_release(&mut input_outcome, released);
        if let Err(error) = released {
            failures.insert(0, release_failure(error));
        }
    }
    let after_input = (!failures.is_empty()).then(|| CliError::device(failures.join("; ")));
    // #463 S1-7: whether the instance was held when the press was made, in `record.warnings`.
    let mut outcome = recorded.map_err(|error| {
        let mut error = with_input_outcome(error, &input_outcome);
        super::record_hold::add_hold_warning_to_failure(&mut error, hold_warning);
        error
    })?;
    outcome.warnings.extend(hold_warning.map(str::to_owned));
    let record = serde_json::to_value(&outcome)
        .map_err(|error| CliError::usage(format!("failed to encode the record: {error}")))?;
    if let Some(mut error) = after_input {
        error.message = format!(
            "{}; the press of step {} was performed and recorded and the screen after it taken",
            error.message, outcome.click.step
        );
        return Err(error.with_details(json!({
            "record": record,
            "input_outcome": input_outcome
        })));
    }
    let mut data = direct_touch_answer(
        &instance,
        command,
        "direct_trusted_manual",
        "not_required_for_manual_control",
        input_outcome,
    );
    data["record"] = record;
    Ok(data)
}

/// The fix round after the train's review: the word of a flag of the screen after a recorded
/// effect whose value is no whole number, the word the Lab's other flags answer for it
/// (`argument_problem` `wrong_type`); `range` stays the word of a value outside its bounds, which
/// the recording checks (`record_after_capture_check`). #463 stage two, train 1, fix round:
/// `record frames` answers its numbers' so too (`session_record.rs`).
pub(crate) const NOT_A_WHOLE_NUMBER: &str = "wrong_type";

/// #463 S1-1: `--after-ms <ms>`, a whole number of milliseconds (the recording checks its bound,
/// 0 to 60000, `range`); anything else is `validation_failed` with `details{argument: "after_ms",
/// reason: "wrong_type", value}` (the fix round after the train's review: the word and the keys of
/// the other three flags). The train of stage one: `session app <verb> --record` reads its
/// `--after-ms` so too.
pub(crate) fn parse_after_ms(value: &str) -> CliOutcome<u64> {
    value.parse::<u64>().map_err(|_| {
        CliError::usage(format!(
            "--after-ms takes a whole number of milliseconds from 0 to \
             {LAB_RECORDING_MAX_NEXT_FRAME_MS}, got {value}"
        ))
        .with_details(json!({
            "argument": "after_ms",
            "reason": NOT_A_WHOLE_NUMBER,
            "value": value
        }))
    })
}

/// #463 S1-2: `flag` (`--interval-ms`, `--still-ms` or `--still-max-ms`) when it is given, a whole
/// number of milliseconds (the recording checks its range and the bound of the captures it allows,
/// `range`); anything else is `validation_failed` with `details{argument, reason: "wrong_type",
/// value}` (the fix round after the train's review). The train of stage one: `session app <verb>
/// --record` reads the three so too.
pub(crate) fn after_capture_ms(
    flags: &FlagArgs,
    flag: &str,
    argument: &str,
) -> CliOutcome<Option<u64>> {
    flags
        .optional(flag)
        .map(|value| {
            value.parse::<u64>().map_err(|_| {
                CliError::usage(format!(
                    "{flag} takes a whole number of milliseconds, got {value}"
                ))
                .with_details(json!({
                    "argument": argument,
                    "reason": NOT_A_WHOLE_NUMBER,
                    "value": value
                }))
            })
        })
        .transpose()
}

/// `error` with the input outcome of the press it followed as the member `input_outcome` of its
/// details; object details keep their members, other details move to `details`.
fn with_input_outcome(mut error: CliError, input_outcome: &Value) -> CliError {
    let mut details = match error.details.take() {
        Some(Value::Object(details)) => details,
        Some(other) => serde_json::Map::from_iter([("details".to_owned(), other)]),
        None => serde_json::Map::new(),
    };
    details
        .entry("input_outcome")
        .or_insert_with(|| input_outcome.clone());
    error.with_details(Value::Object(details))
}

/// #463 S1-1: one input of a press recorded by its command, sent through the Runtime's input proxy
/// (#463 S1-9: the recorded press's, held until the captures after it are done): what the
/// recording takes of it, the input outcome the answer carries (its `close_stage` filled once the
/// lease is released, `note_release`), and the failures after a performed input that leave the
/// press performed (the receipt's encoding; the lease's release is added to them first).
struct SentPress {
    commit: CommitClickRequest,
    input_outcome: Value,
    failures: Vec<String>,
    /// #463 S1-7: the word of `record.warnings` from the status read before the press, on a
    /// connection of its own: none when the instance's scheduling was paused
    /// (`record_hold::hold_warning_read`).
    hold_warning: Option<&'static str>,
}

/// #463 S1-1: sends `action` once, naming `frame`, the Runtime frame it was planned on (fix round
/// 2: the capture taken immediately before it; the Runtime resolves it before it presses, and an
/// instance whose input is the emulator's own presses only on such a frame). A receipt is a
/// performed press (its request, correlation and action ids). An error the Runtime may have pressed
/// before ([`input_not_sent`] does not hold) is an indeterminate one, with the request and
/// correlation ids the error carries (from a received receipt or the failed header read); a refusal
/// that proves nothing was sent was not performed and is answered as `tap` answers it, with
/// `input_outcome` (fix round 3: the frame's being unavailable, [`input_frame_unavailable`],
/// marked so that `record_press` sends the press once more). A connection or lease that fails
/// sends nothing (`input_stage: not_submitted`). #463 S1-9: the input goes through `held`, the
/// recorded press's input proxy, connected (its lease taken) by the press's first input and used
/// again by the one retry; it is not released here (`run_recorded_touch` releases it once the
/// captures after the press are done), so the input outcome's `close_stage` is left for the release
/// (`note_release`, `with_release`).
fn send_recorded_press(
    instance: &str,
    held: &mut Option<RuntimeInputProxy>,
    action: InputAction,
    frame: InputFrameReference,
) -> CliOutcome<SentPress> {
    // #463 S1-7 (`contracts/lab-recording.md`, "A recording holds its instance"): whether the
    // instance is held, one status read before the press (the input proxy's preparation reads
    // none); fix round 2: on a connection of its own, so that a read that fails never leaves the
    // press's connection dead and the press `not_submitted`.
    let hold_warning = super::record_hold::hold_warning_read(instance);
    let proxy = match held {
        Some(proxy) => proxy,
        None => {
            let state_root = runtime_state_root().map_err(input_not_submitted)?;
            let client = RuntimeClient::connect(crate::lab_client_config(state_root))
                .map_err(|error| input_not_submitted(CliError::device(error.to_string())))?;
            held.insert(
                RuntimeInputProxy::connect(client, instance)
                    .map_err(|error| input_not_submitted(CliError::device(error.to_string())))?,
            )
        }
    };
    let operation = proxy.input_with_frame(action, Some(frame));
    let mut input_outcome = json!({
        "input_stage": if operation.is_ok() { "committed" } else { "receipt_unavailable" },
        "input_receipt": Value::Null,
        // #463 S1-9: what the release did, filled when the lease is released (`note_release`).
        "close_stage": Value::Null,
    });
    let mut failures = Vec::new();
    let commit = match operation {
        Ok(receipt) => {
            match serde_json::to_value(&receipt) {
                Ok(value) => input_outcome["input_receipt"] = value,
                Err(error) => {
                    input_outcome["input_receipt_serialization_error"] = json!(error.to_string());
                    failures.push(format!("input receipt serialization failed: {error}"));
                }
            }
            let action_id = match receipt.result() {
                Some(RuntimeResult::InputCommitted { action_id }) => {
                    encoded(action_id, &mut failures)
                }
                _ => None,
            };
            CommitClickRequest {
                effect: ClickEffect::Performed,
                has_failure: false,
                carrier_package: None,
                req_id: encoded(&receipt.request_id(), &mut failures),
                correlation_id: encoded(&receipt.correlation_id(), &mut failures),
                action_id,
                lease_id: None,
                failure: None,
                before: None,
                after: None,
            }
        }
        Err(error) if !input_not_sent(&error) => {
            input_outcome["input_error"] = json!(error.to_string());
            let receipt = error.received_receipt();
            let header = error.receipt_header_io();
            let request_id = receipt
                .map(RuntimeReceipt::request_id)
                .or_else(|| header.and_then(|header| header.request_id().copied()));
            let correlation_id = receipt
                .map(RuntimeReceipt::correlation_id)
                .or_else(|| header.and_then(|header| header.correlation_id().copied()));
            CommitClickRequest {
                effect: ClickEffect::Indeterminate,
                has_failure: false,
                carrier_package: None,
                req_id: request_id.and_then(|id| encoded(&id, &mut failures)),
                correlation_id: correlation_id.and_then(|id| encoded(&id, &mut failures)),
                action_id: None,
                lease_id: None,
                failure: None,
                before: None,
                after: None,
            }
        }
        Err(error) => {
            input_outcome["input_error"] = json!(error.to_string());
            let refusal = CliError::device(error.to_string())
                .with_details(json!({"input_outcome": input_outcome}));
            // Fix round 3: the one refusal `record_press` sends the press again for.
            return Err(if input_frame_unavailable(&error) {
                record_press_frame_unavailable(refusal)
            } else {
                refusal
            });
        }
    };
    Ok(SentPress {
        commit,
        input_outcome,
        failures,
        hold_warning,
    })
}

/// #463 S1-9 (`contracts/lab-recording.md`, "Recording while playing", "The press"): what the
/// release of a recorded press's input lease did, `released`, in the press's input outcome
/// `input_outcome` (an object `send_recorded_press` built): `close_stage` `succeeded` or `failed`,
/// in the place it always had, and with a failure `close_error`, as when the lease was the press's
/// alone.
fn note_release(input_outcome: &mut Value, released: &Result<(), RuntimeClientError>) {
    let stage = if released.is_ok() {
        "succeeded"
    } else {
        "failed"
    };
    input_outcome["close_stage"] = json!(stage);
    if let Err(error) = released {
        input_outcome["close_error"] = json!(error.to_string());
    }
}

/// #463 S1-9: the text of a release of a recorded press's input lease that failed, a failure after
/// the input.
fn release_failure(error: &RuntimeClientError) -> String {
    format!("releasing the input lease failed: {error}")
}

/// #463 S1-9: `error`, which `record_press` answered without a press it could record (a refusal, or
/// the input's own error as `send_recorded_press` answered it), once the press's input lease
/// (`released`, `None` when no lease was taken) is released. When its details hold the input
/// outcome of the press's input (the input's refusal), that outcome gets the release's
/// `close_stage`, as when the lease was the press's alone; any other error is answered as it is,
/// unless the release failed: then its message says so and its details carry `input_outcome`
/// `{close_stage: "failed", close_error}`, so that the failure is never lost.
fn with_release(mut error: CliError, released: Option<Result<(), RuntimeClientError>>) -> CliError {
    let Some(released) = released else {
        return error;
    };
    if let Some(input_outcome) = error
        .details
        .as_mut()
        .and_then(|details| details.get_mut("input_outcome"))
        .filter(|input_outcome| input_outcome.is_object())
    {
        note_release(input_outcome, &released);
        return error;
    }
    match released {
        Ok(()) => error,
        Err(failure) => {
            error.message = format!("{}; {}", error.message, release_failure(&failure));
            with_input_outcome(
                error,
                &json!({"close_stage": "failed", "close_error": failure.to_string()}),
            )
        }
    }
}

/// #463 S1-1 fix round 2 (`contracts/lab-recording.md`, "Recording while playing", "The
/// outcome"): whether an input that failed with `error` was certainly not sent to the device,
/// decided by the error's result class (Workflow #338 R6) and the state of the receipt it carries:
/// a usage or safety refusal the Runtime denied (the request, the frame or the foreground refused,
/// a lease, queue or preemption rule) is answered before the input's intent is recorded, so
/// nothing was pressed. After every other error the Runtime's own record of the input may say it
/// is indeterminate: a device failure of the backend on a physical instance (`runtime-host`
/// `host/input.rs`, `backend_failure_effect`), a Runtime failure after the device write, an input
/// its operation check stopped mid-press (a failed receipt of the lease's own safety code), or an
/// answer the client never read (uncertain). `do --capture --record` records the Runtime's own
/// effect of its press the same way (anything but `not_performed` is on record).
fn input_not_sent(error: &RuntimeClientError) -> bool {
    matches!(
        error.disposition(),
        RuntimeClientErrorClass::Usage | RuntimeClientErrorClass::Safety
    ) && error
        .received_receipt()
        .is_some_and(|receipt| receipt.state() == RuntimeReceiptState::Denied)
}

/// #463 S1-1 fix round 3: the code of the Runtime's refusal of an input whose frame it no longer
/// holds (`runtime-host` `host/input.rs`, the frame's resolution before the input's intent).
const INPUT_FRAME_UNAVAILABLE: &str = "input_frame_unavailable";

/// #463 S1-1 fix round 3 (`contracts/lab-recording.md`, "Recording while playing", "The screen
/// before"): whether an input failed with `error` because the Runtime no longer held the frame it
/// names (another keyless capture, a capture by a lease holder or the capture backend reopening
/// dropped it between the command's capture and its input): the receipt the Runtime denied it
/// with, whose outcome's code is the frame's being unavailable. Nothing was sent. The one place
/// that tells this refusal from every other, by the receipt's state and the outcome's code the
/// client hands over (the coordinator's ruling of fix round 3: no class or word of the outcome
/// separates it); `record_press` sends the press once more on a new capture for it, and for no
/// other refusal.
fn input_frame_unavailable(error: &RuntimeClientError) -> bool {
    error
        .received_receipt()
        .is_some_and(|receipt| receipt.state() == RuntimeReceiptState::Denied)
        && error
            .receipt_outcome()
            .is_some_and(|outcome| outcome.code().as_str() == INPUT_FRAME_UNAVAILABLE)
}

/// `value` as the recording keeps it; one that does not encode leaves its member empty and its
/// failure in `failures`, a failure after the input.
fn encoded<T: serde::Serialize>(value: &T, failures: &mut Vec<String>) -> Option<OpaqueJson> {
    match OpaqueJson::from_serializable(value) {
        Ok(value) => Some(value),
        Err(error) => {
            failures.push(error.message);
            None
        }
    }
}

/// #463 S1-1: the caller's clock and the Runtime's capture of a press recorded by `tap`, `swipe` or
/// `long-tap --record`: each frame is taken as `capture --record` takes it (the `ObserveReadonly`
/// read), on the Runtime's capture endpoint of the command's instance (fix round 3: no local
/// device configuration); fix round 2: with the reference of the frame the Runtime committed for
/// its observation, which the press is sent with. #463 S1-2: the clock's readings are the time
/// since `origin`, the command's start of the recording's work (a monotonic clock, as `wait` sleeps
/// on it).
struct PressFrames<'a> {
    endpoint: &'a runtime_capture_backend::RuntimeCaptureEndpoint,
    origin: Instant,
}

impl RecordFramePorts for PressFrames<'_> {
    fn wait(&mut self, duration: Duration) {
        std::thread::sleep(duration);
    }

    fn elapsed(&mut self) -> Duration {
        self.origin.elapsed()
    }

    fn capture(&mut self) -> CliOutcome<CapturedFrame> {
        runtime_captured_frame(self.endpoint)
    }
}

/// #463 S1-1: one frame through the Runtime's capture endpoint `endpoint` as `capture --record`
/// takes it (the `ObserveReadonly` read, no local device configuration), with the reference of the
/// frame the Runtime committed for its observation. A recorded press takes its frames so
/// (`PressFrames`); the fix round after the train's review: `session app <verb> --record` too.
pub(crate) fn runtime_captured_frame(
    endpoint: &runtime_capture_backend::RuntimeCaptureEndpoint,
) -> CliOutcome<CapturedFrame> {
    let captured = capture_observation_for_command(endpoint)?;
    let png = captured
        .frame
        .png_for_artifact()
        .map_err(|err| CliError::device(err.to_string()))?;
    Ok(CapturedFrame {
        frame: AttachFrameRequest {
            png,
            source: "lab_capture".to_string(),
            runtime_artifact: None,
            capture_backend: Some(captured.frame.backend_name.as_str().to_string()),
            freshness: Some(OpaqueJson::from_serializable(&captured.freshness)?),
        },
        input_frame: captured.input_frame,
    })
}

pub(crate) fn run_direct_input(
    global: &GlobalOptions,
    command: &str,
    args: &[String],
) -> CliOutcome<Value> {
    let (command, config) = (|| -> CliOutcome<_> {
        let flags = FlagArgs::parse(args)?;
        reject_legacy_session_routing(&flags)?;
        let command = DirectInputCommand::parse(command, &flags)?;
        let config = read_user_config()?;
        Ok((command, config))
    })()
    .map_err(input_not_submitted)?;
    if global.dry_run {
        let instance = resolve_instance_id(global, &config).map_err(input_not_submitted)?;
        return Ok(crate::dry_run_gate::input_preview(
            &instance,
            command.to_json(),
        ));
    }
    let (mut backend, instance_alias) =
        open_cli_runtime_input_proxy(global, &config).map_err(input_not_submitted)?;
    let operation = command.run(&mut backend);
    let close = backend.close();
    let input_outcome = finish_direct_input(operation, close)?;
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
        "action": command.to_json(),
        "input_outcome": input_outcome
    }))
}

// The command owns its input receipt before closing the proxy.
fn finish_direct_input(
    operation: actingcommand_device::DeviceResult<RuntimeReceipt>,
    close: actingcommand_device::DeviceResult<()>,
) -> CliOutcome<Value> {
    let mut outcome = json!({
        "input_stage": if operation.is_ok() { "committed" } else { "receipt_unavailable" },
        "input_receipt": Value::Null,
        "close_stage": if close.is_ok() { "succeeded" } else { "failed" },
    });
    if let Err(error) = &operation {
        outcome["input_error"] = json!(error.to_string());
    }
    if let Err(error) = &close {
        outcome["close_error"] = json!(error.to_string());
    }
    let serialization_error = match &operation {
        Ok(receipt) => match serde_json::to_value(receipt) {
            Ok(value) => {
                outcome["input_receipt"] = value;
                None
            }
            Err(error) => Some(error.to_string()),
        },
        Err(_) => None,
    };
    let combined = combine_operation_and_close(operation.map(|_| ()), close)
        .map_err(|error| CliError::device(error.to_string()));
    if let Some(error) = serialization_error {
        outcome["input_receipt_serialization_error"] = json!(&error);
        let failure = match combined {
            Err(primary) => primary,
            Ok(()) => CliError::device(format!("input receipt serialization failed: {error}")),
        };
        return Err(failure.with_details(json!({"input_outcome": outcome})));
    }
    match combined {
        Ok(()) => Ok(outcome),
        Err(error) => Err(error.with_details(json!({"input_outcome": outcome}))),
    }
}

fn input_not_submitted(mut error: CliError) -> CliError {
    let outcome = json!({"input_stage": "not_submitted", "input_receipt": Value::Null});
    let mut details = match error.details.take() {
        Some(Value::Object(details)) => details,
        Some(original) => serde_json::Map::from_iter([("original_details".to_owned(), original)]),
        None => serde_json::Map::new(),
    };
    details.insert("input_outcome".to_owned(), outcome);
    error.details = Some(Value::Object(details));
    error
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DirectInputCommand {
    Key(String),
    Text(String),
}

/// Workflow #434 LAB-5B2: the flags `key` and `text` take: none, they take positional arguments
/// only.
pub(crate) const DIRECT_INPUT_FLAGS: [&str; 0] = [];

impl DirectInputCommand {
    pub(crate) fn parse(command: &str, flags: &FlagArgs) -> CliOutcome<Self> {
        flags.reject_flags(command, &DIRECT_INPUT_FLAGS)?;
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

    fn run(
        &self,
        backend: &mut runtime_input_backend::RuntimeInputBackend,
    ) -> actingcommand_device::DeviceResult<RuntimeReceipt> {
        let action = match self {
            Self::Key(key) => InputAction::Key { key: key.clone() },
            Self::Text(text) => InputAction::Text { text: text.clone() },
        };
        backend.input_receipt(action)
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
        let action_flags = FlagArgs::from_parts(BTreeMap::new(), action_args);
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

    fn run(
        &self,
        backend: &mut runtime_input_backend::RuntimeInputBackend,
    ) -> actingcommand_device::DeviceResult<()> {
        match self {
            Self::Touch(command) => command.run(backend).map(|_| ()),
            Self::Input(command) => command.run(backend).map(|_| ()),
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
