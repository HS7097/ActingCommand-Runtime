// SPDX-License-Identifier: AGPL-3.0-only

use super::{
    CliError, CliOutcome, FlagArgs, GlobalOptions, app_state_root, current_unix_ms, device_config,
    effective_resource_root, finish_semantic_result_with_ledger, parse_optional_duration_ms,
    read_user_config, semantic_ledger_context,
};
use actingcommand_contract::{DriveRecord, LedgerProjection};
use actingcommand_device::{
    CaptureBackend, DeviceError, InputBackend, SelectedTouchBackend, create_capture_backend,
    create_touch_backend,
};
use actingcommand_lab::{
    CaptureBackendFactory, CaptureBackendRequest, Clock, ConfigSource, EnvDetectRequest,
    EnvMarkerResolutionRequest, EnvResolveRequest, EnvScopeRequest, EnvStatusRequest,
    InputBackendAttemptReport, InputBackendFactory, InputBackendObservation, InputBackendReport,
    InputBackendRequest, InputHandshakeReport, Lab, LabError, LabPorts, LabState, LedgerSink,
    UserConfig,
};
use serde::Serialize;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

pub(super) use actingcommand_lab::ResolvedEnvValue;

pub(super) fn run_detect(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    let mut ledger = semantic_ledger_context("detect", global, args);
    let result = (|| -> CliOutcome<Value> {
        let (request, config, input_metadata) = detect_request(global, &flags)?;
        let mut lab = build_lab(&request.scope, config, input_metadata)?;
        let response = lab.detect_env(request)?;
        record_detect_drive(&mut ledger, &response)?;
        serialize_response(response)
    })();
    finish_semantic_result_with_ledger(global, ledger, result)
}

pub(super) fn run_env(
    subcommand: &str,
    global: &GlobalOptions,
    args: &[String],
) -> CliOutcome<Value> {
    match subcommand {
        "resolve" => run_env_resolve(global, args),
        "status" => run_env_status(global, args),
        other => Err(CliError::usage(format!(
            "unknown env command: {other}; expected resolve or status"
        ))),
    }
}

fn run_env_resolve(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    let mut ledger = semantic_ledger_context("env-resolve", global, args);
    let result = (|| -> CliOutcome<Value> {
        let (request, config) = resolve_request(global, &flags)?;
        let mut lab = build_lab(&request.scope, config, None)?;
        let response = lab.resolve_env(request)?;
        ledger.record_drive(json!({
            "stage": "env_resolved",
            "detector_id": response.detector_id,
            "instance_id": response.instance_id,
            "source_result": response.source_result,
            "keys": response.keys.iter().map(|value| json!({
                "key": value.key,
                "value": value.value,
                "confidence": value.confidence,
                "source": value.source
            })).collect::<Vec<_>>()
        }))?;
        serialize_response(response)
    })();
    finish_semantic_result_with_ledger(global, ledger, result)
}

fn run_env_status(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    let (request, config) = status_request(global, &flags)?;
    let mut lab = build_lab(&request.scope, config, None)?;
    serialize_response(lab.env_status(request)?)
}

pub(super) fn resolve_env_markers_in_value(
    global: &GlobalOptions,
    flags: &FlagArgs,
    resource_root: &Path,
    value: &mut Value,
) -> CliOutcome<Vec<ResolvedEnvValue>> {
    let request = EnvMarkerResolutionRequest {
        resource_root: resource_root.to_path_buf(),
        instance: flags
            .optional("--instance")
            .or_else(|| global.instance.clone()),
        game: flags.optional("--game").or_else(|| global.game.clone()),
        server: flags.optional("--server").or_else(|| global.server.clone()),
        env_task: flags.optional("--env-task"),
    };
    let mut lab = build_marker_lab()?;
    lab.resolve_env_markers(request, value)
}

fn build_marker_lab() -> CliOutcome<Lab<AppLabPorts>> {
    Lab::new(
        AppLabPorts {
            input: AppInputFactory {
                input_metadata: None,
            },
            capture: AppCaptureFactory,
            ledger: CliOwnedLedger,
            clock: SystemClock,
            config: AppConfigSource {
                config: UserConfig::default(),
                state_root: None,
            },
        },
        LabState::open(".")?,
    )
}

fn detect_request(
    global: &GlobalOptions,
    flags: &FlagArgs,
) -> CliOutcome<(EnvDetectRequest, UserConfig, Option<InputFactoryMetadata>)> {
    let config = read_user_config()?;
    let scope = command_scope(global, flags, &config, "env detection")?;
    let capture = flags.bool("--capture");
    let device = capture
        .then(|| device_config(global, &config))
        .transpose()?;
    let input_metadata = device.as_ref().map(InputFactoryMetadata::from_device);
    Ok((
        EnvDetectRequest {
            scope,
            task: flags.required("--task")?,
            scene_path: flags.optional_path("--scene"),
            capture_config: device
                .as_ref()
                .map(|device| device.capture_backend_config()),
            touch_config: device.as_ref().map(|device| device.touch_backend_config()),
            require_fresh: flags.bool("--require-fresh"),
            fresh_delay: parse_optional_duration_ms(flags, "--fresh-delay-ms", 160)?,
            dry_run: global.dry_run || flags.bool("--dry-run"),
        },
        config,
        input_metadata,
    ))
}

fn resolve_request(
    global: &GlobalOptions,
    flags: &FlagArgs,
) -> CliOutcome<(EnvResolveRequest, UserConfig)> {
    let config = read_user_config()?;
    Ok((
        EnvResolveRequest {
            scope: command_scope(global, flags, &config, "env detection")?,
            task: flags.required("--task")?,
            input: flags
                .optional("--path")
                .or_else(|| flags.optional("--value"))
                .or_else(|| flags.positionals.first().cloned()),
            key: flags.optional("--key"),
        },
        config,
    ))
}

fn status_request(
    global: &GlobalOptions,
    flags: &FlagArgs,
) -> CliOutcome<(EnvStatusRequest, UserConfig)> {
    let config = read_user_config()?;
    Ok((
        EnvStatusRequest {
            scope: command_scope(global, flags, &config, "env detection")?,
            task: flags.required("--task")?,
        },
        config,
    ))
}

fn command_scope(
    global: &GlobalOptions,
    flags: &FlagArgs,
    config: &UserConfig,
    label: &str,
) -> CliOutcome<EnvScopeRequest> {
    let resource_root = flags
        .optional_path("--resource-root")
        .or_else(|| effective_resource_root(global, config))
        .ok_or_else(|| {
            CliError::usage(format!(
                "{label} requires --resource-root or config.resource_root"
            ))
        })?;
    let game = flags
        .optional("--game")
        .or_else(|| global.game.clone())
        .ok_or_else(|| CliError::usage(format!("{label} requires --game")))?;
    let instance = flags
        .optional("--instance")
        .or_else(|| global.instance.clone())
        .ok_or_else(|| CliError::usage(format!("{label} requires --instance")))?;
    Ok(EnvScopeRequest {
        resource_root,
        state_root: app_state_root()?,
        instance,
        game,
        server: flags.optional("--server").or_else(|| global.server.clone()),
    })
}

fn build_lab(
    scope: &EnvScopeRequest,
    config: UserConfig,
    input_metadata: Option<InputFactoryMetadata>,
) -> CliOutcome<Lab<AppLabPorts>> {
    let state = LabState::open(&scope.state_root)?;
    Lab::new(
        AppLabPorts {
            input: AppInputFactory { input_metadata },
            capture: AppCaptureFactory,
            ledger: CliOwnedLedger,
            clock: SystemClock,
            config: AppConfigSource {
                config,
                state_root: Some(scope.state_root.clone()),
            },
        },
        state,
    )
}

fn record_detect_drive(
    ledger: &mut actingcommand_lab::SemanticLedgerContext,
    response: &actingcommand_lab::EnvDetectResponse,
) -> CliOutcome<()> {
    if response.status == "planned" {
        return ledger.record_drive(json!({
            "stage": "env_detection_steps_planned",
            "detector_id": response.detector_id,
            "detector_version": response.detector_version,
            "instance_id": response.instance_id,
            "steps": response.steps
        }));
    }
    let result = response.result.as_ref().ok_or_else(|| {
        CliError::device("detected env response is missing its typed detection result")
    })?;
    let detections = result.detected_facts();
    ledger.record_drive(json!({
        "stage": "env_detected",
        "detector_id": response.detector_id,
        "detector_version": response.detector_version,
        "instance_id": response.instance_id,
        "result_path": response.result_path,
        "detections": detections.iter().map(|value| json!({
            "key": value.key,
            "value": value.value,
            "confidence": value.confidence,
            "source": value.source
        })).collect::<Vec<_>>()
    }))
}

fn serialize_response<T: Serialize>(response: T) -> CliOutcome<Value> {
    serde_json::to_value(response)
        .map_err(|error| CliError::device(format!("failed to serialize Lab response: {error}")))
}

struct AppLabPorts {
    input: AppInputFactory,
    capture: AppCaptureFactory,
    ledger: CliOwnedLedger,
    clock: SystemClock,
    config: AppConfigSource,
}

impl LabPorts for AppLabPorts {
    type InputFactory = AppInputFactory;
    type CaptureFactory = AppCaptureFactory;
    type Ledger = CliOwnedLedger;
    type Time = SystemClock;
    type Config = AppConfigSource;

    fn input_factory(&self) -> &Self::InputFactory {
        &self.input
    }

    fn capture_factory(&self) -> &Self::CaptureFactory {
        &self.capture
    }

    fn ledger(&mut self) -> &mut Self::Ledger {
        &mut self.ledger
    }

    fn clock(&self) -> &Self::Time {
        &self.clock
    }

    fn config(&self) -> &Self::Config {
        &self.config
    }
}

#[derive(Clone)]
struct InputFactoryMetadata {
    adb_source: String,
    adb_warning: Option<String>,
}

impl InputFactoryMetadata {
    fn from_device(device: &super::DeviceRuntimeConfig) -> Self {
        Self {
            adb_source: device.adb_source.as_str().to_string(),
            adb_warning: device.adb_warning.clone(),
        }
    }
}

struct AppInputFactory {
    input_metadata: Option<InputFactoryMetadata>,
}

impl InputBackendFactory for AppInputFactory {
    fn open(&self, request: InputBackendRequest) -> Result<Box<dyn InputBackend>, LabError> {
        let metadata = self
            .input_metadata
            .clone()
            .ok_or_else(|| LabError::device("env detection input metadata was not configured"))?;
        let selected = create_touch_backend(request.config)
            .map_err(|error| LabError::device(error.to_string()))?;
        let backend = ObservedInputBackend {
            selected,
            observation: request.observation,
            metadata,
        };
        backend.publish_report()?;
        Ok(Box::new(backend))
    }
}

struct ObservedInputBackend {
    selected: SelectedTouchBackend,
    observation: Option<InputBackendObservation>,
    metadata: InputFactoryMetadata,
}

impl ObservedInputBackend {
    fn publish_report(&self) -> Result<(), LabError> {
        if let Some(observation) = &self.observation {
            observation.record(input_report(&self.selected, &self.metadata))?;
        }
        Ok(())
    }

    fn finish_operation(
        &mut self,
        operation: actingcommand_device::DeviceResult<()>,
    ) -> actingcommand_device::DeviceResult<()> {
        self.publish_report()
            .map_err(|error| DeviceError::fatal(error.to_string()))?;
        operation
    }
}

impl InputBackend for ObservedInputBackend {
    fn tap(&mut self, x: i32, y: i32) -> actingcommand_device::DeviceResult<()> {
        let operation = self.selected.tap(x, y);
        self.finish_operation(operation)
    }

    fn long_tap(
        &mut self,
        x: i32,
        y: i32,
        duration_ms: u64,
    ) -> actingcommand_device::DeviceResult<()> {
        let operation = self.selected.long_tap(x, y, duration_ms);
        self.finish_operation(operation)
    }

    fn swipe(
        &mut self,
        x1: i32,
        y1: i32,
        x2: i32,
        y2: i32,
        duration_ms: u64,
    ) -> actingcommand_device::DeviceResult<()> {
        let operation = self.selected.swipe(x1, y1, x2, y2, duration_ms);
        self.finish_operation(operation)
    }

    fn key(&mut self, key: &str) -> actingcommand_device::DeviceResult<()> {
        let operation = self.selected.key(key);
        self.finish_operation(operation)
    }

    fn text(&mut self, text: &str) -> actingcommand_device::DeviceResult<()> {
        let operation = self.selected.text(text);
        self.finish_operation(operation)
    }

    fn reset(&mut self) -> actingcommand_device::DeviceResult<()> {
        let operation = self.selected.reset();
        self.finish_operation(operation)
    }

    fn close(&mut self) -> actingcommand_device::DeviceResult<()> {
        let close = self.selected.close();
        self.publish_report()
            .map_err(|error| DeviceError::fatal(error.to_string()))?;
        close
    }
}

fn input_report(
    selected: &SelectedTouchBackend,
    metadata: &InputFactoryMetadata,
) -> InputBackendReport {
    let diagnostics = selected.diagnostics();
    InputBackendReport {
        backend: selected.backend_name().as_str().to_string(),
        requested_backend: diagnostics.requested.as_str().to_string(),
        adb_source: metadata.adb_source.clone(),
        adb_warning: metadata.adb_warning.clone(),
        attempts: diagnostics
            .attempts
            .iter()
            .map(|attempt| InputBackendAttemptReport {
                attempt_id: attempt.attempt_id,
                backend: attempt.backend.as_str().to_string(),
                ok: attempt.ok,
                elapsed_ms: attempt.elapsed_ms,
                action: attempt.action.clone(),
                fallback_backend: attempt
                    .fallback_backend
                    .map(|backend| backend.as_str().to_string()),
                error_reason: attempt.error_reason.clone(),
                selected: attempt.selected,
            })
            .collect(),
        warnings: diagnostics.warnings.clone(),
        serial: selected.serial().to_string(),
        device_state: selected.device_info().state.clone(),
        screen_size: selected.device_info().screen_size.clone(),
        handshake: selected
            .handshake_info()
            .map(|handshake| InputHandshakeReport {
                max_contacts: handshake.max_contacts,
                max_x: handshake.max_x,
                max_y: handshake.max_y,
                max_pressure: handshake.max_pressure,
                pid: handshake.pid.clone(),
            }),
    }
}

struct AppCaptureFactory;

impl CaptureBackendFactory for AppCaptureFactory {
    fn open(&self, request: CaptureBackendRequest) -> Result<Box<dyn CaptureBackend>, LabError> {
        create_capture_backend(request.config)
            .map(|selected| selected.backend)
            .map_err(|error| LabError::device(error.to_string()))
    }
}

struct CliOwnedLedger;

impl LedgerSink for CliOwnedLedger {
    fn append_drive<T: Serialize>(&mut self, _record: &DriveRecord<T>) -> Result<(), LabError> {
        Err(LabError::device(
            "semantic ledger is owned by the CLI adapter during A3 migration",
        ))
    }

    fn finish<T: Serialize>(&mut self, _response: &T) -> Result<LedgerProjection, LabError> {
        Err(LabError::device(
            "semantic ledger is owned by the CLI adapter during A3 migration",
        ))
    }
}

struct SystemClock;

impl Clock for SystemClock {
    fn now_unix_ms(&self) -> Result<u64, LabError> {
        Ok(current_unix_ms())
    }

    fn sleep(&self, duration: Duration) {
        thread::sleep(duration);
    }
}

struct AppConfigSource {
    config: UserConfig,
    state_root: Option<PathBuf>,
}

impl ConfigSource for AppConfigSource {
    fn load(&self) -> Result<UserConfig, LabError> {
        Ok(self.config.clone())
    }

    fn state_root(&self) -> Result<PathBuf, LabError> {
        match &self.state_root {
            Some(path) => Ok(path.clone()),
            None => app_state_root(),
        }
    }
}
