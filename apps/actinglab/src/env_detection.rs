// SPDX-License-Identifier: AGPL-3.0-only

use super::{
    CliError, CliOutcome, FlagArgs, GlobalOptions, app_state_root, current_unix_ms,
    effective_resource_root, finish_semantic_result_with_ledger, parse_optional_duration_ms,
    read_user_config, runtime_state_root, semantic_ledger_context,
};
use actingcommand_contract::InputAction;
use actingcommand_device::{
    AdbConfig, CaptureBackend, CaptureBackendConfig, DeviceError, DeviceTarget, InputBackend,
    MaaTouchConfig, TouchBackendConfig,
};
use actingcommand_lab::{
    CaptureBackendFactory, CaptureBackendRequest, Clock, ConfigSource, EnvDetectRequest,
    EnvMarkerResolutionRequest, EnvResolveRequest, EnvScopeRequest, EnvStatusRequest,
    InputBackendAttemptReport, InputBackendFactory, InputBackendObservation, InputBackendReport,
    InputBackendRequest, Lab, LabError, LabPorts, LabState, LedgerEventEntry, LedgerLastResort,
    LedgerReadback, LedgerRecordEntry, LedgerSessionHeader, LedgerSink, RunLedgerSessionRequest,
    SemanticInputExecutor, UserConfig,
};
use actingcommand_ledger::{LabLedger, LastResortError, LedgerRecord, write_last_resort_error};
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig, RuntimeInputProxy};
use serde::{Serialize, de::DeserializeOwned};
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
        let mut lab = build_lab(
            &request.scope,
            config,
            input_metadata.clone(),
            input_metadata,
        )?;
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
        let mut lab = build_lab(&request.scope, config, None, None)?;
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
    let mut lab = build_lab(&request.scope, config, None, None)?;
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
    let mut lab = build_readonly_lab()?;
    lab.resolve_env_markers(request, value)
}

pub(super) fn build_readonly_lab() -> CliOutcome<Lab<AppLabPorts>> {
    build_app_lab(UserConfig::default(), None, AppCaptureAuthority::Disabled)
}

pub(super) fn build_readonly_lab_for_capture(
    instance_alias: Option<&str>,
) -> CliOutcome<Lab<AppLabPorts>> {
    let authority = match instance_alias {
        Some(instance_alias) => AppCaptureAuthority::Runtime(
            super::runtime_capture_backend::RuntimeCaptureEndpoint::new(
                instance_alias.to_string(),
                runtime_state_root()?,
            ),
        ),
        None => AppCaptureAuthority::Disabled,
    };
    build_app_lab(UserConfig::default(), None, authority)
}

pub(super) fn build_control_lab(config: UserConfig) -> CliOutcome<Lab<AppLabPorts>> {
    build_app_lab(
        config,
        None,
        AppCaptureAuthority::RuntimeByInstance(runtime_state_root()?),
    )
}

pub(super) fn build_drive_lab(
    config: UserConfig,
    instance_alias: Option<&str>,
    enable_input: bool,
) -> CliOutcome<Lab<AppLabPorts>> {
    let runtime_metadata = instance_alias
        .map(|alias| InputFactoryMetadata::new(alias.to_string()))
        .transpose()?;
    let capture_authority =
        runtime_metadata
            .clone()
            .map_or(AppCaptureAuthority::Disabled, |metadata| {
                AppCaptureAuthority::Runtime(
                    super::runtime_capture_backend::RuntimeCaptureEndpoint::new(
                        metadata.instance_alias,
                        metadata.runtime_state_root,
                    ),
                )
            });
    let input_metadata = enable_input.then_some(runtime_metadata).flatten();
    build_app_lab(config, input_metadata, capture_authority)
}

fn build_app_lab(
    config: UserConfig,
    input_metadata: Option<InputFactoryMetadata>,
    capture_authority: AppCaptureAuthority,
) -> CliOutcome<Lab<AppLabPorts>> {
    Lab::new(
        AppLabPorts {
            semantic_input: AppSemanticInputExecutor {
                input_metadata: input_metadata.clone(),
            },
            input: AppInputFactory { input_metadata },
            capture: AppCaptureFactory {
                authority: capture_authority,
            },
            ledger: AppLedgerSink,
            clock: SystemClock,
            config: AppConfigSource {
                config,
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
    let input_metadata = capture
        .then(|| InputFactoryMetadata::new(scope.instance.clone()))
        .transpose()?;
    Ok((
        EnvDetectRequest {
            scope,
            task: flags.required("--task")?,
            scene_path: flags.optional_path("--scene"),
            capture_config: capture.then(runtime_capture_port_config),
            touch_config: capture.then(runtime_touch_port_config),
            require_fresh: flags.bool("--require-fresh"),
            fresh_delay: parse_optional_duration_ms(flags, "--fresh-delay-ms", 160)?,
            dry_run: global.dry_run || flags.bool("--dry-run"),
        },
        config,
        input_metadata,
    ))
}

pub(super) fn runtime_capture_port_config() -> CaptureBackendConfig {
    CaptureBackendConfig::new(AdbConfig::default(), DeviceTarget::default())
}

fn runtime_touch_port_config() -> TouchBackendConfig {
    TouchBackendConfig::new(
        AdbConfig::default(),
        DeviceTarget::default(),
        MaaTouchConfig::default(),
    )
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
    capture_metadata: Option<InputFactoryMetadata>,
) -> CliOutcome<Lab<AppLabPorts>> {
    let state = LabState::open(&scope.state_root)?;
    Lab::new(
        AppLabPorts {
            semantic_input: AppSemanticInputExecutor {
                input_metadata: input_metadata.clone(),
            },
            input: AppInputFactory { input_metadata },
            capture: AppCaptureFactory {
                authority: capture_metadata.map_or(AppCaptureAuthority::Disabled, |metadata| {
                    AppCaptureAuthority::Runtime(
                        super::runtime_capture_backend::RuntimeCaptureEndpoint::new(
                            metadata.instance_alias,
                            metadata.runtime_state_root,
                        ),
                    )
                }),
            },
            ledger: AppLedgerSink,
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

pub(super) struct AppLabPorts {
    input: AppInputFactory,
    semantic_input: AppSemanticInputExecutor,
    capture: AppCaptureFactory,
    ledger: AppLedgerSink,
    clock: SystemClock,
    config: AppConfigSource,
}

impl LabPorts for AppLabPorts {
    type InputFactory = AppInputFactory;
    type SemanticInput = AppSemanticInputExecutor;
    type CaptureFactory = AppCaptureFactory;
    type Ledger = AppLedgerSink;
    type Time = SystemClock;
    type Config = AppConfigSource;

    fn input_factory(&self) -> &Self::InputFactory {
        &self.input
    }

    fn semantic_input(&self) -> &Self::SemanticInput {
        &self.semantic_input
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
    instance_alias: String,
    runtime_state_root: PathBuf,
}

impl InputFactoryMetadata {
    fn new(instance_alias: String) -> CliOutcome<Self> {
        Ok(Self {
            instance_alias,
            runtime_state_root: runtime_state_root()?,
        })
    }
}

pub(super) struct AppSemanticInputExecutor {
    input_metadata: Option<InputFactoryMetadata>,
}

impl SemanticInputExecutor for AppSemanticInputExecutor {
    fn execute(&self, action: InputAction) -> Result<InputBackendReport, LabError> {
        let metadata = self
            .input_metadata
            .as_ref()
            .ok_or_else(|| LabError::device("Runtime input metadata was not configured"))?;
        let client = RuntimeClient::connect(RuntimeClientConfig::new(
            &metadata.runtime_state_root,
            actingcommand_contract::EventActor::Lab,
            actingcommand_contract::EventSource::Lab,
        ))
        .map_err(|error| LabError::device(error.to_string()))?;
        let mut proxy = RuntimeInputProxy::connect(client, &metadata.instance_alias)
            .map_err(|error| LabError::device(error.to_string()))?;
        let operation = proxy.input(action);
        let close = proxy.close();
        match (operation, close) {
            (Ok(()), Ok(())) => Ok(input_report()),
            (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(LabError::device(error.to_string())),
            (Err(operation), Err(close)) => Err(LabError::device(format!(
                "{operation}; Runtime input proxy close also failed: {close}"
            ))),
        }
    }
}

pub(super) struct AppInputFactory {
    input_metadata: Option<InputFactoryMetadata>,
}

impl InputBackendFactory for AppInputFactory {
    fn open(&self, request: InputBackendRequest) -> Result<Box<dyn InputBackend>, LabError> {
        let InputBackendRequest {
            instance_alias,
            config: _runtime_owned_touch_config,
            observation,
        } = request;
        let metadata = instance_alias
            .map(InputFactoryMetadata::new)
            .transpose()?
            .or_else(|| self.input_metadata.clone())
            .ok_or_else(|| LabError::device("Runtime input metadata was not configured"))?;
        let client = RuntimeClient::connect(RuntimeClientConfig::new(
            &metadata.runtime_state_root,
            actingcommand_contract::EventActor::Lab,
            actingcommand_contract::EventSource::Lab,
        ))
        .map_err(|error| LabError::device(error.to_string()))?;
        let proxy = super::runtime_input_backend::RuntimeInputBackend::connect(
            client,
            &metadata.instance_alias,
        )
        .map_err(|error| LabError::device(error.to_string()))?;
        let backend = ObservedInputBackend { proxy, observation };
        backend.publish_report()?;
        Ok(Box::new(backend))
    }
}

struct ObservedInputBackend {
    proxy: super::runtime_input_backend::RuntimeInputBackend,
    observation: Option<InputBackendObservation>,
}

impl ObservedInputBackend {
    fn publish_report(&self) -> Result<(), LabError> {
        if let Some(observation) = &self.observation {
            observation.record(input_report())?;
        }
        Ok(())
    }

    fn finish_operation(
        &mut self,
        operation: actingcommand_device::DeviceResult<()>,
    ) -> actingcommand_device::DeviceResult<()> {
        let report = self
            .publish_report()
            .map_err(|error| DeviceError::fatal(error.to_string()));
        combine_device_results(operation, report)
    }
}

impl InputBackend for ObservedInputBackend {
    fn tap(&mut self, x: i32, y: i32) -> actingcommand_device::DeviceResult<()> {
        let operation = self.proxy.tap(x, y);
        self.finish_operation(operation)
    }

    fn long_tap(
        &mut self,
        x: i32,
        y: i32,
        duration_ms: u64,
    ) -> actingcommand_device::DeviceResult<()> {
        let operation = self.proxy.long_tap(x, y, duration_ms);
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
        let operation = self.proxy.swipe(x1, y1, x2, y2, duration_ms);
        self.finish_operation(operation)
    }

    fn key(&mut self, key: &str) -> actingcommand_device::DeviceResult<()> {
        let operation = self.proxy.key(key);
        self.finish_operation(operation)
    }

    fn text(&mut self, text: &str) -> actingcommand_device::DeviceResult<()> {
        let operation = self.proxy.text(text);
        self.finish_operation(operation)
    }

    fn reset(&mut self) -> actingcommand_device::DeviceResult<()> {
        let operation = self.proxy.reset();
        self.finish_operation(operation)
    }

    fn close(&mut self) -> actingcommand_device::DeviceResult<()> {
        let close = self.proxy.close();
        let report = self
            .publish_report()
            .map_err(|error| DeviceError::fatal(error.to_string()));
        combine_device_results(close, report)
    }
}

fn input_report() -> InputBackendReport {
    InputBackendReport {
        backend: "runtime_proxy".to_string(),
        requested_backend: "runtime_owned".to_string(),
        adb_source: "runtime_owned".to_string(),
        adb_warning: None,
        attempts: vec![InputBackendAttemptReport {
            attempt_id: 1,
            backend: "runtime_proxy".to_string(),
            ok: true,
            elapsed_ms: 0,
            action: Some("lease_acquire".to_string()),
            fallback_backend: None,
            error_reason: None,
            selected: true,
        }],
        warnings: Vec::new(),
        serial: "<runtime-owned>".to_string(),
        device_state: "runtime_owned".to_string(),
        screen_size: "<runtime-owned>".to_string(),
        handshake: None,
    }
}

fn combine_device_results(
    operation: actingcommand_device::DeviceResult<()>,
    report: actingcommand_device::DeviceResult<()>,
) -> actingcommand_device::DeviceResult<()> {
    match (operation, report) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(operation), Err(report)) => Err(DeviceError::fatal(format!(
            "{operation}; input report also failed: {report}"
        ))),
    }
}

enum AppCaptureAuthority {
    Disabled,
    Runtime(super::runtime_capture_backend::RuntimeCaptureEndpoint),
    RuntimeByInstance(PathBuf),
}

pub(super) struct AppCaptureFactory {
    authority: AppCaptureAuthority,
}

impl CaptureBackendFactory for AppCaptureFactory {
    fn open(&self, request: CaptureBackendRequest) -> Result<Box<dyn CaptureBackend>, LabError> {
        match &self.authority {
            AppCaptureAuthority::Disabled => Err(LabError::device(
                "Runtime capture metadata was not configured",
            )),
            AppCaptureAuthority::Runtime(endpoint) => {
                super::runtime_capture_backend::open_runtime_capture(endpoint.clone(), request)
            }
            AppCaptureAuthority::RuntimeByInstance(state_root) => {
                let instance_alias = request.instance_alias.clone().ok_or_else(|| {
                    LabError::device("Runtime capture request is missing instance alias")
                })?;
                let endpoint = super::runtime_capture_backend::RuntimeCaptureEndpoint::new(
                    instance_alias,
                    state_root.clone(),
                );
                super::runtime_capture_backend::open_runtime_capture(endpoint, request)
            }
        }
    }
}

pub(super) struct AppLedgerSink;

pub(super) struct AppRunLedgerSession {
    ledger: Option<LabLedger>,
}

impl LedgerSink for AppLedgerSink {
    type RunSession = AppRunLedgerSession;

    fn run_session(&mut self) -> Self::RunSession {
        AppRunLedgerSession { ledger: None }
    }

    fn start_run_session(
        session: &mut Self::RunSession,
        request: RunLedgerSessionRequest,
    ) -> CliOutcome<PathBuf> {
        if session.ledger.is_some() {
            return Err(LabError::package_invalid(
                "invalid lab logging input: runtime ledger session is already started",
            ));
        }
        let header =
            decode_ledger_json(&request.header().encoded_json()?, "ledger session header")?;
        let ledger = LabLedger::create_runtime_shard(
            request.run_root(),
            request.run_id(),
            request.instance(),
            header,
        )
        .map_err(app_ledger_error)?;
        let path = ledger.ledger_path().to_path_buf();
        session.ledger = Some(ledger);
        Ok(path)
    }

    fn append_run_record(
        session: &mut Self::RunSession,
        record: LedgerRecordEntry,
    ) -> CliOutcome<()> {
        let record = decode_record(record)?;
        app_run_ledger_mut(session)?
            .append(record)
            .map_err(app_ledger_error)
    }

    fn append_run_event(session: &mut Self::RunSession, event: LedgerEventEntry) -> CliOutcome<()> {
        let event = decode_ledger_json(&event.encoded_json()?, "ledger event")?;
        app_run_ledger_mut(session)?
            .append_event(event)
            .map_err(app_ledger_error)
    }

    fn sync_run_session(session: &Self::RunSession) -> CliOutcome<()> {
        app_run_ledger(session)?.sync().map_err(app_ledger_error)
    }

    fn read_run_session(session: &Self::RunSession) -> CliOutcome<LedgerReadback> {
        let read =
            LabLedger::read(app_run_ledger(session)?.ledger_path()).map_err(app_ledger_error)?;
        let header = read
            .header
            .map(|header| {
                let encoded = encode_ledger_json(&header, "ledger session header")?;
                LedgerSessionHeader::from_json(&encoded)
            })
            .transpose()?;
        let events = read
            .events
            .into_iter()
            .map(|event| {
                let encoded = encode_ledger_json(&event, "ledger event")?;
                LedgerEventEntry::from_json(&encoded)
            })
            .collect::<CliOutcome<Vec<_>>>()?;
        let records = read
            .records
            .into_iter()
            .map(|record| {
                let encoded = encode_ledger_json(&record, "ledger record")?;
                LedgerRecordEntry::from_json(&encoded)
            })
            .collect::<CliOutcome<Vec<_>>>()?;
        Ok(LedgerReadback::new(
            header,
            events,
            records,
            read.skipped_corrupt_lines,
        ))
    }

    fn write_run_last_resort(
        run_root: Option<&Path>,
        error: &LedgerLastResort,
    ) -> CliOutcome<PathBuf> {
        let error: LastResortError =
            decode_ledger_json(&error.encoded_json()?, "last-resort ledger error")?;
        write_last_resort_error(run_root, &error).map_err(app_ledger_error)
    }
}

fn app_run_ledger(session: &AppRunLedgerSession) -> CliOutcome<&LabLedger> {
    session.ledger.as_ref().ok_or_else(|| {
        LabError::package_invalid("invalid lab logging input: runtime ledger handle is unavailable")
    })
}

fn app_run_ledger_mut(session: &mut AppRunLedgerSession) -> CliOutcome<&mut LabLedger> {
    session.ledger.as_mut().ok_or_else(|| {
        LabError::package_invalid("invalid lab logging input: runtime ledger handle is unavailable")
    })
}

pub(super) fn decode_record(record: LedgerRecordEntry) -> CliOutcome<LedgerRecord> {
    decode_ledger_json(&record.encoded_json()?, "ledger record")
}

fn encode_ledger_json<T: Serialize>(value: &T, label: &str) -> CliOutcome<String> {
    serde_json::to_string(value)
        .map_err(|error| LabError::package_invalid(format!("failed to encode {label}: {error}")))
}

fn decode_ledger_json<T: DeserializeOwned>(encoded: &str, label: &str) -> CliOutcome<T> {
    serde_json::from_str(encoded)
        .map_err(|error| LabError::package_invalid(format!("failed to decode {label}: {error}")))
}

fn app_ledger_error(error: impl std::fmt::Display) -> LabError {
    LabError::package_invalid(error.to_string())
}

pub(super) struct SystemClock;

impl Clock for SystemClock {
    fn now_unix_ms(&self) -> Result<u64, LabError> {
        Ok(current_unix_ms())
    }

    fn sleep(&self, duration: Duration) {
        thread::sleep(duration);
    }
}

pub(super) struct AppConfigSource {
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
