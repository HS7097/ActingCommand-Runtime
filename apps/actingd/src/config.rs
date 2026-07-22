// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_contract::{ApplicationLifecycleAction, ContainedTaskRequest, InstanceId};
use actingcommand_device::{
    AdbConfig, CaptureBackend, CaptureBackendChoice, CaptureBackendConfig, CaptureBackendName,
    DeviceError, DeviceResult, DeviceTarget, Frame, InputBackend, MaaTouchConfig, MinitouchConfig,
    PixelFormat, TouchBackendChoice, TouchBackendConfig,
};
use actingcommand_policy::{
    CatalogDocumentSource, CatalogSources, EvaluationFacts, EvaluationResources, MAX_APPROVAL_REFS,
    MAX_CATALOG_BYTES, MAX_DOCUMENT_BYTES, MAX_REFERENCES_PER_TASK, MAX_TASKS, compile_catalog,
};
use actingcommand_runtime_host::{
    AgentDispatcherConfig, ExecutionBackendProvider, ExecutionBackendRegistration,
    ExecutionBackendRegistry, PerformanceMonitorConfig, PolicyInputSnapshot, ProcedureBinding,
    ProcedureManifest, ResolvedExecutionInstance, RuntimeHostConfig,
};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::Duration;

const CONFIG_SCHEMA_VERSION: &str = "actingcommand.actingd.config.v1";
const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
const MAX_TIMEOUT_MS: u64 = 120_000;
const MAX_FIXTURE_FRAMES: usize = 32;
const MAX_FIXTURE_FRAME_BYTES: usize = 16 * 1024 * 1024;
const MAX_FIXTURE_RESIDENT_BYTES: usize = 32 * 1024 * 1024;
const MAX_FIXTURE_INPUTS: u16 = 32;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ActingdConfigFile {
    schema_version: String,
    state_root: PathBuf,
    bind_host: String,
    #[serde(default)]
    bind_port: u16,
    secret_fingerprint_salt: String,
    #[serde(default)]
    governance_capability: Option<String>,
    #[serde(default)]
    agent_dispatcher: Option<AgentDispatcherConfigFile>,
    #[serde(default)]
    policy: Option<PolicyConfigFile>,
    instances: Vec<InstanceConfig>,
    #[serde(skip)]
    source_root: PathBuf,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentDispatcherConfigFile {
    max_attempts: u16,
    max_session_ms: u64,
    max_projection_events: u16,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PolicyConfigFile {
    facts: EvaluationFacts,
    resources: EvaluationResources,
    catalog: PolicyCatalogConfigFile,
    catalog_approval_ids: Vec<String>,
    procedure_manifest: Vec<ProcedureBindingConfigFile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PolicyCatalogConfigFile {
    tasks: PathBuf,
    pools: PathBuf,
    activity: PathBuf,
    timeline: PathBuf,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProcedureBindingConfigFile {
    procedure_ref: String,
    package_digest: String,
    operation_id: String,
    yield_points: Vec<String>,
    #[serde(default)]
    scheduled_execution: Option<ScheduledExecutionConfigFile>,
}

#[derive(Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
enum ScheduledExecutionConfigFile {
    FixtureSimulation {
        #[serde(default)]
        package_path: Option<PathBuf>,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InstanceConfig {
    alias: String,
    instance_id: InstanceId,
    #[serde(default)]
    application_id: Option<String>,
    #[serde(default)]
    adb_path: Option<String>,
    #[serde(default)]
    serial: Option<String>,
    #[serde(default)]
    host: Option<String>,
    #[serde(default)]
    port: Option<u16>,
    #[serde(default)]
    connect: Option<bool>,
    #[serde(default)]
    touch_backend: Option<String>,
    #[serde(default)]
    capture_backend: Option<String>,
    #[serde(default)]
    command_timeout_ms: Option<u64>,
    #[serde(default)]
    maatouch_local_path: Option<PathBuf>,
    #[serde(default)]
    minitouch_local_path: Option<PathBuf>,
    #[serde(default)]
    push_touch_tool: Option<bool>,
    #[serde(default)]
    handshake_timeout_ms: Option<u64>,
    #[serde(default)]
    shutdown_timeout_ms: Option<u64>,
    #[serde(default)]
    tap_hold_ms: Option<u64>,
    #[serde(default)]
    fixture_backend: Option<FixtureBackendConfigFile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureBackendConfigFile {
    frames: Vec<FixtureFrameConfigFile>,
    max_inputs: u16,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureFrameConfigFile {
    width: u32,
    height: u32,
    rgb: Vec<u8>,
}

pub(super) struct RuntimeAssembly {
    pub(super) host: RuntimeHostConfig,
    pub(super) registry: ConfiguredExecutionBackendRegistry,
    pub(super) policy: Option<PolicyBootstrap>,
}

pub(super) struct PolicyBootstrap {
    pub(super) state_root: PathBuf,
    pub(super) governance_capability: String,
    pub(super) catalog_approval_ids: Vec<String>,
    pub(super) catalog: CatalogSources,
    pub(super) scheduled_tasks: BTreeMap<String, ContainedTaskRequest>,
}

pub(super) enum ConfiguredExecutionBackendRegistry {
    Device(ExecutionBackendRegistry),
    Fixture(FixtureExecutionBackendRegistry),
}

pub(super) struct FixtureExecutionBackendRegistry {
    instances: BTreeMap<String, FixtureExecutionBackend>,
}

struct FixtureExecutionBackend {
    instance_id: InstanceId,
    frames: Vec<Frame>,
    max_inputs: u16,
}

enum ConfiguredInstanceBackend {
    Device(Box<ExecutionBackendRegistration>),
    Fixture {
        alias: String,
        backend: FixtureExecutionBackend,
    },
}

type ScheduledProcedureTask = (String, ContainedTaskRequest);
type AssembledProcedureBinding = (ProcedureBinding, Option<ScheduledProcedureTask>);

pub(super) fn load(path: &Path) -> Result<ActingdConfigFile, &'static str> {
    let metadata = fs::metadata(path).map_err(|_| "config_unavailable")?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_CONFIG_BYTES {
        return Err("config_size_invalid");
    }
    let bytes = fs::read(path).map_err(|_| "config_read_failed")?;
    let mut config =
        serde_json::from_slice::<ActingdConfigFile>(&bytes).map_err(|_| "config_decode_failed")?;
    config.source_root = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    Ok(config)
}

impl ActingdConfigFile {
    pub(super) fn assemble(self) -> Result<RuntimeAssembly, &'static str> {
        if self.schema_version != CONFIG_SCHEMA_VERSION
            || self.state_root.as_os_str().is_empty()
            || !(16..=1024).contains(&self.secret_fingerprint_salt.len())
            || self.governance_capability.as_ref().is_some_and(|value| {
                !(actingcommand_contract::MIN_GOVERNANCE_CAPABILITY_BYTES
                    ..=actingcommand_contract::MAX_GOVERNANCE_CAPABILITY_BYTES)
                    .contains(&value.len())
                    || value.chars().any(char::is_control)
            })
        {
            return Err("config_invalid");
        }
        let bind_host = self
            .bind_host
            .parse::<IpAddr>()
            .map_err(|_| "bind_host_invalid")?;
        if !bind_host.is_loopback() {
            return Err("bind_host_not_loopback");
        }
        let registrations = self
            .instances
            .into_iter()
            .map(InstanceConfig::backend)
            .collect::<Result<Vec<_>, _>>()?;
        let registry = ConfiguredExecutionBackendRegistry::new(registrations)?;
        let policy = self
            .policy
            .map(|policy| policy.assemble(&self.source_root))
            .transpose()?;
        if policy
            .as_ref()
            .is_some_and(|policy| !policy.scheduled_tasks.is_empty())
            && !registry.is_fixture_simulation()
        {
            return Err("fixture_simulation_requires_fixture_backend");
        }
        let policy_state_root = self.state_root.clone();
        let policy_governance_capability = self.governance_capability.clone();
        let mut host =
            RuntimeHostConfig::new(self.state_root, self.secret_fingerprint_salt.as_bytes())
                .with_bind_address(SocketAddr::new(bind_host, self.bind_port))
                .with_performance_monitor(PerformanceMonitorConfig::default());
        if let Some(capability) = self.governance_capability {
            host = host.with_governance_capability(capability);
        }
        if let Some(dispatcher) = self.agent_dispatcher {
            host = host.with_agent_dispatcher(dispatcher.runtime_config()?);
        }
        let policy = if let Some(policy) = policy {
            let governance_capability =
                policy_governance_capability.ok_or("policy_governance_capability_missing")?;
            host = host
                .with_policy_inputs(policy.inputs)
                .with_procedure_manifest(policy.procedure_manifest);
            Some(PolicyBootstrap {
                state_root: policy_state_root,
                governance_capability,
                catalog_approval_ids: policy.catalog_approval_ids,
                catalog: policy.catalog,
                scheduled_tasks: policy.scheduled_tasks,
            })
        } else {
            None
        };
        Ok(RuntimeAssembly {
            host,
            registry,
            policy,
        })
    }
}

struct PolicyAssembly {
    inputs: PolicyInputSnapshot,
    procedure_manifest: ProcedureManifest,
    catalog_approval_ids: Vec<String>,
    catalog: CatalogSources,
    scheduled_tasks: BTreeMap<String, ContainedTaskRequest>,
}

impl PolicyConfigFile {
    fn assemble(self, source_root: &Path) -> Result<PolicyAssembly, &'static str> {
        if self.procedure_manifest.is_empty() || self.procedure_manifest.len() > MAX_TASKS {
            return Err("procedure_manifest_size_invalid");
        }
        let mut bindings = Vec::with_capacity(self.procedure_manifest.len());
        let mut scheduled_tasks = BTreeMap::new();
        for configured in self.procedure_manifest {
            let (binding, scheduled_task) = configured.binding(source_root)?;
            if let Some((procedure_ref, request)) = scheduled_task
                && scheduled_tasks.insert(procedure_ref, request).is_some()
            {
                return Err("procedure_task_duplicate");
            }
            bindings.push(binding);
        }
        let procedure_manifest =
            ProcedureManifest::new(bindings).map_err(|_| "procedure_manifest_invalid")?;
        let catalog = self.catalog.sources(source_root)?;
        let compiled = compile_catalog(&catalog).map_err(|_| "policy_catalog_compile_failed")?;
        let configured_approvals = self
            .catalog_approval_ids
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        let expected_approvals = compiled
            .catalog()
            .tasks
            .catalog
            .approval_refs
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        if self.catalog_approval_ids.is_empty()
            || self.catalog_approval_ids.len() > MAX_APPROVAL_REFS
            || configured_approvals.len() != self.catalog_approval_ids.len()
            || configured_approvals != expected_approvals
        {
            return Err("policy_catalog_approval_mismatch");
        }
        Ok(PolicyAssembly {
            inputs: PolicyInputSnapshot::new(self.facts, self.resources),
            procedure_manifest,
            catalog_approval_ids: self.catalog_approval_ids,
            catalog,
            scheduled_tasks,
        })
    }
}

impl PolicyCatalogConfigFile {
    fn sources(self, source_root: &Path) -> Result<CatalogSources, &'static str> {
        let sources = CatalogSources {
            tasks: read_catalog_document(source_root, &self.tasks)?,
            pools: read_catalog_document(source_root, &self.pools)?,
            activity: read_catalog_document(source_root, &self.activity)?,
            timeline: read_catalog_document(source_root, &self.timeline)?,
        };
        let total_bytes = [
            &sources.tasks,
            &sources.pools,
            &sources.activity,
            &sources.timeline,
        ]
        .into_iter()
        .try_fold(0_usize, |total, source| {
            total.checked_add(source.bytes.len())
        })
        .ok_or("policy_catalog_size_invalid")?;
        if total_bytes > MAX_CATALOG_BYTES {
            return Err("policy_catalog_size_invalid");
        }
        Ok(sources)
    }
}

impl ProcedureBindingConfigFile {
    fn binding(self, source_root: &Path) -> Result<AssembledProcedureBinding, &'static str> {
        let Self {
            procedure_ref,
            package_digest,
            operation_id,
            yield_points,
            scheduled_execution,
        } = self;
        if yield_points.len() > MAX_REFERENCES_PER_TASK {
            return Err("procedure_binding_size_invalid");
        }
        let binding = ProcedureBinding::new(
            procedure_ref.clone(),
            package_digest.clone(),
            operation_id,
            yield_points,
        )
        .map_err(|_| "procedure_binding_invalid")?;
        let scheduled_task = match scheduled_execution {
            None => None,
            Some(ScheduledExecutionConfigFile::FixtureSimulation { package_path }) => {
                let package_path = package_path.ok_or("procedure_package_path_missing")?;
                let path = if package_path.is_absolute() {
                    package_path
                } else {
                    source_root.join(package_path)
                };
                let path = fs::canonicalize(path).map_err(|_| "procedure_package_unavailable")?;
                let metadata = fs::metadata(&path).map_err(|_| "procedure_package_unavailable")?;
                if !metadata.is_file() {
                    return Err("procedure_package_not_regular");
                }
                let expected_sha256 = package_digest
                    .strip_prefix("sha256:")
                    .ok_or("procedure_package_digest_invalid")?;
                let request =
                    ContainedTaskRequest::new(path.to_string_lossy().into_owned(), expected_sha256)
                        .map_err(|_| "procedure_task_request_invalid")?;
                Some((procedure_ref, request))
            }
        };
        Ok((binding, scheduled_task))
    }
}

fn read_catalog_document(
    source_root: &Path,
    configured_path: &Path,
) -> Result<CatalogDocumentSource, &'static str> {
    if configured_path.as_os_str().is_empty() {
        return Err("policy_catalog_path_invalid");
    }
    let path = if configured_path.is_absolute() {
        configured_path.to_path_buf()
    } else {
        source_root.join(configured_path)
    };
    let metadata = fs::metadata(&path).map_err(|_| "policy_catalog_unavailable")?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_DOCUMENT_BYTES as u64 {
        return Err("policy_catalog_document_size_invalid");
    }
    let bytes = fs::read(&path).map_err(|_| "policy_catalog_read_failed")?;
    Ok(CatalogDocumentSource::new(
        format!("file://{}", path.to_string_lossy().replace('\\', "/")),
        bytes,
    ))
}

impl AgentDispatcherConfigFile {
    fn runtime_config(self) -> Result<AgentDispatcherConfig, &'static str> {
        AgentDispatcherConfig::new(
            self.max_attempts,
            self.max_session_ms,
            self.max_projection_events,
        )
        .map_err(|_| "agent_dispatcher_config_invalid")
    }
}

impl InstanceConfig {
    fn backend(self) -> Result<ConfiguredInstanceBackend, &'static str> {
        if self.fixture_backend.is_some() {
            self.fixture_backend()
        } else {
            self.device_backend()
        }
    }

    fn device_backend(self) -> Result<ConfiguredInstanceBackend, &'static str> {
        let adb_path = self.adb_path.ok_or("instance_config_invalid")?;
        let host = self.host.unwrap_or_else(default_device_host);
        let port = self.port.unwrap_or_else(default_device_port);
        let connect = self.connect.unwrap_or_else(enabled);
        let touch_backend = self.touch_backend.ok_or("touch_backend_invalid")?;
        let capture_backend = self.capture_backend.ok_or("capture_backend_invalid")?;
        if adb_path.trim().is_empty()
            || host.trim().is_empty()
            || port == 0
            || self
                .serial
                .as_ref()
                .is_some_and(|value| value.trim().is_empty())
        {
            return Err("instance_config_invalid");
        }
        let application_id = self
            .application_id
            .filter(|value| !value.trim().is_empty())
            .ok_or("application_identity_missing")?;
        let requested =
            TouchBackendChoice::parse(&touch_backend).map_err(|_| "touch_backend_invalid")?;
        if matches!(
            requested,
            TouchBackendChoice::Auto | TouchBackendChoice::AutoFastest
        ) {
            return Err("touch_backend_must_be_explicit");
        }
        let capture_requested =
            CaptureBackendChoice::parse(&capture_backend).map_err(|_| "capture_backend_invalid")?;
        if matches!(
            capture_requested,
            CaptureBackendChoice::Auto | CaptureBackendChoice::AutoFastest
        ) {
            return Err("capture_backend_must_be_explicit");
        }
        let mut adb = AdbConfig {
            adb_path,
            ..AdbConfig::default()
        };
        if let Some(timeout) = bounded_duration(self.command_timeout_ms)? {
            adb.command_timeout = timeout;
        }
        let target = DeviceTarget {
            serial: self.serial,
            host,
            port,
            connect,
        };
        let mut maatouch = MaaTouchConfig::default();
        let mut minitouch = MinitouchConfig::default();
        if let Some(path) = self.maatouch_local_path {
            maatouch.local_path = path;
        }
        if let Some(path) = self.minitouch_local_path {
            minitouch.local_path = path;
        }
        if let Some(push) = self.push_touch_tool {
            maatouch.push = push;
            minitouch.push = push;
        }
        if let Some(timeout) = bounded_duration(self.handshake_timeout_ms)? {
            maatouch.handshake_timeout = timeout;
            minitouch.handshake_timeout = timeout;
        }
        if let Some(timeout) = bounded_duration(self.shutdown_timeout_ms)? {
            maatouch.shutdown_timeout = timeout;
            minitouch.shutdown_timeout = timeout;
        }
        if let Some(hold) = bounded_duration(self.tap_hold_ms)? {
            maatouch.tap_hold = hold;
            minitouch.tap_hold = hold;
        }
        let capture = CaptureBackendConfig::new(adb.clone(), target.clone())
            .with_requested(capture_requested);
        let touch = TouchBackendConfig::new(adb, target, maatouch)
            .with_minitouch_config(minitouch)
            .with_requested(requested);
        ExecutionBackendRegistration::new(
            self.alias,
            self.instance_id,
            application_id,
            touch,
            capture,
        )
        .map(Box::new)
        .map(ConfiguredInstanceBackend::Device)
        .map_err(|_| "instance_registration_invalid")
    }

    fn fixture_backend(self) -> Result<ConfiguredInstanceBackend, &'static str> {
        if self.application_id.is_some()
            || self.adb_path.is_some()
            || self.serial.is_some()
            || self.host.is_some()
            || self.port.is_some()
            || self.connect.is_some()
            || self.touch_backend.is_some()
            || self.capture_backend.is_some()
            || self.command_timeout_ms.is_some()
            || self.maatouch_local_path.is_some()
            || self.minitouch_local_path.is_some()
            || self.push_touch_tool.is_some()
            || self.handshake_timeout_ms.is_some()
            || self.shutdown_timeout_ms.is_some()
            || self.tap_hold_ms.is_some()
        {
            return Err("fixture_device_fields_forbidden");
        }
        let configured = self.fixture_backend.ok_or("fixture_backend_missing")?;
        if self.alias.trim().is_empty()
            || configured.frames.is_empty()
            || configured.frames.len() > MAX_FIXTURE_FRAMES
            || configured.max_inputs > MAX_FIXTURE_INPUTS
        {
            return Err("fixture_backend_invalid");
        }
        let mut resident_bytes = 0_usize;
        let frames = configured
            .frames
            .into_iter()
            .map(|frame| {
                let expected_bytes = usize::try_from(frame.width)
                    .ok()
                    .and_then(|width| {
                        usize::try_from(frame.height)
                            .ok()
                            .and_then(|height| width.checked_mul(height))
                    })
                    .and_then(|pixels| pixels.checked_mul(3))
                    .ok_or("fixture_frame_size_invalid")?;
                if expected_bytes == 0
                    || expected_bytes > MAX_FIXTURE_FRAME_BYTES
                    || frame.rgb.len() != expected_bytes
                {
                    return Err("fixture_frame_size_invalid");
                }
                resident_bytes = resident_bytes
                    .checked_add(expected_bytes)
                    .ok_or("fixture_resident_size_invalid")?;
                if resident_bytes > MAX_FIXTURE_RESIDENT_BYTES {
                    return Err("fixture_resident_size_invalid");
                }
                Frame::from_pixels(
                    frame.width,
                    frame.height,
                    frame.rgb,
                    PixelFormat::Rgb8,
                    CaptureBackendName::FixtureSimulation,
                )
                .map_err(|_| "fixture_frame_invalid")
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ConfiguredInstanceBackend::Fixture {
            alias: self.alias,
            backend: FixtureExecutionBackend {
                instance_id: self.instance_id,
                frames,
                max_inputs: configured.max_inputs,
            },
        })
    }
}

impl ConfiguredExecutionBackendRegistry {
    fn new(backends: Vec<ConfiguredInstanceBackend>) -> Result<Self, &'static str> {
        if backends.is_empty() {
            return Err("execution_registry_invalid");
        }
        let mut devices = Vec::new();
        let mut fixtures = BTreeMap::new();
        for backend in backends {
            match backend {
                ConfiguredInstanceBackend::Device(registration) => devices.push(*registration),
                ConfiguredInstanceBackend::Fixture { alias, backend } => {
                    if fixtures.insert(alias, backend).is_some() {
                        return Err("execution_registry_invalid");
                    }
                }
            }
        }
        match (devices.is_empty(), fixtures.is_empty()) {
            (false, true) => ExecutionBackendRegistry::new(devices)
                .map(Self::Device)
                .map_err(|_| "execution_registry_invalid"),
            (true, false) => Ok(Self::Fixture(FixtureExecutionBackendRegistry {
                instances: fixtures,
            })),
            _ => Err("execution_backend_mode_mixed"),
        }
    }

    fn is_fixture_simulation(&self) -> bool {
        matches!(self, Self::Fixture(_))
    }
}

impl ExecutionBackendProvider for ConfiguredExecutionBackendRegistry {
    fn instance_aliases(&self) -> Vec<String> {
        match self {
            Self::Device(registry) => registry.instance_aliases(),
            Self::Fixture(registry) => registry.instance_aliases(),
        }
    }

    fn resolve(&self, instance_alias: &str) -> Option<ResolvedExecutionInstance> {
        match self {
            Self::Device(registry) => registry.resolve(instance_alias),
            Self::Fixture(registry) => registry.resolve(instance_alias),
        }
    }

    fn open_input(&self, instance_alias: &str) -> DeviceResult<Box<dyn InputBackend>> {
        match self {
            Self::Device(registry) => registry.open_input(instance_alias),
            Self::Fixture(registry) => registry.open_input(instance_alias),
        }
    }

    fn open_capture(&self, instance_alias: &str) -> DeviceResult<Box<dyn CaptureBackend>> {
        match self {
            Self::Device(registry) => registry.open_capture(instance_alias),
            Self::Fixture(registry) => registry.open_capture(instance_alias),
        }
    }

    fn control_application(
        &self,
        instance_alias: &str,
        action: ApplicationLifecycleAction,
    ) -> DeviceResult<()> {
        match self {
            Self::Device(registry) => registry.control_application(instance_alias, action),
            Self::Fixture(registry) => registry.control_application(instance_alias, action),
        }
    }
}

impl ExecutionBackendProvider for FixtureExecutionBackendRegistry {
    fn instance_aliases(&self) -> Vec<String> {
        self.instances.keys().cloned().collect()
    }

    fn resolve(&self, instance_alias: &str) -> Option<ResolvedExecutionInstance> {
        self.instances
            .get(instance_alias)
            .map(|backend| ResolvedExecutionInstance::fixture_simulation(backend.instance_id))
    }

    fn open_input(&self, instance_alias: &str) -> DeviceResult<Box<dyn InputBackend>> {
        let backend = self
            .instances
            .get(instance_alias)
            .ok_or_else(|| DeviceError::fatal("fixture instance is unknown"))?;
        Ok(Box::new(FixtureInputBackend {
            remaining: backend.max_inputs,
            closed: false,
        }))
    }

    fn open_capture(&self, instance_alias: &str) -> DeviceResult<Box<dyn CaptureBackend>> {
        let backend = self
            .instances
            .get(instance_alias)
            .ok_or_else(|| DeviceError::fatal("fixture instance is unknown"))?;
        Ok(Box::new(FixtureCaptureBackend {
            frames: backend.frames.clone().into(),
        }))
    }

    fn control_application(
        &self,
        _instance_alias: &str,
        _action: ApplicationLifecycleAction,
    ) -> DeviceResult<()> {
        Err(DeviceError::fatal(
            "fixture application control is forbidden",
        ))
    }
}

struct FixtureCaptureBackend {
    frames: VecDeque<Frame>,
}

impl CaptureBackend for FixtureCaptureBackend {
    fn capture(&mut self) -> DeviceResult<Frame> {
        self.frames
            .pop_front()
            .ok_or_else(|| DeviceError::fatal("fixture capture exhausted"))
    }
}

struct FixtureInputBackend {
    remaining: u16,
    closed: bool,
}

impl FixtureInputBackend {
    fn consume(&mut self) -> DeviceResult<()> {
        if self.closed || self.remaining == 0 {
            return Err(DeviceError::fatal("fixture input budget exhausted"));
        }
        self.remaining -= 1;
        Ok(())
    }
}

impl InputBackend for FixtureInputBackend {
    fn tap(&mut self, _x: i32, _y: i32) -> DeviceResult<()> {
        self.consume()
    }

    fn long_tap(&mut self, _x: i32, _y: i32, _duration_ms: u64) -> DeviceResult<()> {
        self.consume()
    }

    fn swipe(
        &mut self,
        _x1: i32,
        _y1: i32,
        _x2: i32,
        _y2: i32,
        _duration_ms: u64,
    ) -> DeviceResult<()> {
        self.consume()
    }

    fn key(&mut self, _key: &str) -> DeviceResult<()> {
        self.consume()
    }

    fn text(&mut self, _text: &str) -> DeviceResult<()> {
        self.consume()
    }

    fn reset(&mut self) -> DeviceResult<()> {
        self.consume()
    }

    fn close(&mut self) -> DeviceResult<()> {
        self.closed = true;
        Ok(())
    }
}

fn bounded_duration(value: Option<u64>) -> Result<Option<Duration>, &'static str> {
    match value {
        Some(value) if value == 0 || value > MAX_TIMEOUT_MS => Err("timeout_invalid"),
        Some(value) => Ok(Some(Duration::from_millis(value))),
        None => Ok(None),
    }
}

fn default_device_host() -> String {
    "127.0.0.1".to_string()
}

const fn default_device_port() -> u16 {
    16384
}

const fn enabled() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use actingcommand_contract::IdentifierIssuer;
    use serde_json::json;
    use tempfile::TempDir;

    #[test]
    fn typed_config_builds_loopback_host_and_registry() {
        let root = TempDir::new().expect("tempdir");
        let id = IdentifierIssuer::new()
            .expect("issuer")
            .mint_instance_id()
            .expect("instance id");
        let value = json!({
            "schema_version": CONFIG_SCHEMA_VERSION,
            "state_root": root.path(),
            "bind_host": "127.0.0.1",
            "bind_port": 0,
            "secret_fingerprint_salt": "0123456789abcdef",
            "instances": [{
                "alias": "node.a",
                "instance_id": id.transport(),
                "application_id": "neutral.application",
                "adb_path": "adb",
                "port": 16384,
                "touch_backend": "maatouch",
                "capture_backend": "adb",
                "push_touch_tool": false
            }]
        });
        let config = serde_json::from_value::<ActingdConfigFile>(value).expect("typed config");
        let assembly = config.assemble().expect("runtime assembly");
        assert_eq!(assembly.host.state_root(), root.path());
    }

    #[test]
    fn missing_application_identity_is_rejected_before_runtime_start() {
        let id = IdentifierIssuer::new()
            .expect("issuer")
            .mint_instance_id()
            .expect("instance id");
        let value = json!({
            "schema_version": CONFIG_SCHEMA_VERSION,
            "state_root": "state",
            "bind_host": "127.0.0.1",
            "secret_fingerprint_salt": "0123456789abcdef",
            "instances": [{
                "alias": "neutral.instance",
                "instance_id": id.transport(),
                "adb_path": "adb",
                "port": 16384,
                "touch_backend": "maatouch",
                "capture_backend": "adb"
            }]
        });
        let config = serde_json::from_value::<ActingdConfigFile>(value).expect("typed config");
        assert_eq!(
            config.assemble().err(),
            Some("application_identity_missing")
        );
    }

    #[test]
    fn unknown_config_field_is_rejected() {
        let value = json!({
            "schema_version": CONFIG_SCHEMA_VERSION,
            "state_root": "state",
            "bind_host": "127.0.0.1",
            "secret_fingerprint_salt": "0123456789abcdef",
            "instances": [],
            "unexpected": true
        });
        assert!(serde_json::from_value::<ActingdConfigFile>(value).is_err());
    }

    #[test]
    fn automatic_touch_fallback_is_rejected_at_the_process_boundary() {
        let id = IdentifierIssuer::new()
            .expect("issuer")
            .mint_instance_id()
            .expect("instance id");
        let value = json!({
            "schema_version": CONFIG_SCHEMA_VERSION,
            "state_root": "state",
            "bind_host": "127.0.0.1",
            "secret_fingerprint_salt": "0123456789abcdef",
            "instances": [{
                "alias": "node.a",
                "instance_id": id.transport(),
                "application_id": "neutral.application",
                "adb_path": "adb",
                "touch_backend": "auto",
                "capture_backend": "adb"
            }]
        });
        let config = serde_json::from_value::<ActingdConfigFile>(value).expect("typed config");
        assert_eq!(
            config.assemble().err(),
            Some("touch_backend_must_be_explicit")
        );
    }

    #[test]
    fn automatic_capture_fallback_is_rejected_at_the_process_boundary() {
        let id = IdentifierIssuer::new()
            .expect("issuer")
            .mint_instance_id()
            .expect("instance id");
        let value = json!({
            "schema_version": CONFIG_SCHEMA_VERSION,
            "state_root": "state",
            "bind_host": "127.0.0.1",
            "secret_fingerprint_salt": "0123456789abcdef",
            "instances": [{
                "alias": "node.a",
                "instance_id": id.transport(),
                "application_id": "neutral.application",
                "adb_path": "adb",
                "touch_backend": "maatouch",
                "capture_backend": "auto"
            }]
        });
        let config = serde_json::from_value::<ActingdConfigFile>(value).expect("typed config");
        assert_eq!(
            config.assemble().err(),
            Some("capture_backend_must_be_explicit")
        );
    }

    #[test]
    fn fixture_backend_is_device_free_and_has_a_bounded_input_budget() {
        let id = IdentifierIssuer::new()
            .expect("issuer")
            .mint_instance_id()
            .expect("instance id");
        let fixture = |max_inputs| {
            json!({
                "schema_version": CONFIG_SCHEMA_VERSION,
                "state_root": "state",
                "bind_host": "127.0.0.1",
                "secret_fingerprint_salt": "0123456789abcdef",
                "instances": [{
                    "alias": "neutral.fixture",
                    "instance_id": id.transport(),
                    "fixture_backend": {
                        "frames": [{"width": 1, "height": 1, "rgb": [1, 2, 3]}],
                        "max_inputs": max_inputs
                    }
                }]
            })
        };

        let config = serde_json::from_value::<ActingdConfigFile>(fixture(MAX_FIXTURE_INPUTS))
            .expect("typed fixture config");
        let assembly = config.assemble().expect("bounded fixture assembly");
        assert!(matches!(
            assembly.registry,
            ConfiguredExecutionBackendRegistry::Fixture(_)
        ));

        let config = serde_json::from_value::<ActingdConfigFile>(fixture(MAX_FIXTURE_INPUTS + 1))
            .expect("typed fixture config");
        assert_eq!(config.assemble().err(), Some("fixture_backend_invalid"));
    }

    #[test]
    fn fixture_backend_rejects_device_fields() {
        let id = IdentifierIssuer::new()
            .expect("issuer")
            .mint_instance_id()
            .expect("instance id");
        let value = json!({
            "schema_version": CONFIG_SCHEMA_VERSION,
            "state_root": "state",
            "bind_host": "127.0.0.1",
            "secret_fingerprint_salt": "0123456789abcdef",
            "instances": [{
                "alias": "neutral.fixture",
                "instance_id": id.transport(),
                "adb_path": "must-not-open",
                "fixture_backend": {
                    "frames": [{"width": 1, "height": 1, "rgb": [1, 2, 3]}],
                    "max_inputs": 0
                }
            }]
        });
        let config = serde_json::from_value::<ActingdConfigFile>(value).expect("typed config");
        assert_eq!(
            config.assemble().err(),
            Some("fixture_device_fields_forbidden")
        );
    }

    #[test]
    fn agent_dispatcher_configuration_is_explicit_and_bounded() {
        let id = IdentifierIssuer::new()
            .expect("issuer")
            .mint_instance_id()
            .expect("instance id");
        let base = json!({
            "schema_version": CONFIG_SCHEMA_VERSION,
            "state_root": "state",
            "bind_host": "127.0.0.1",
            "secret_fingerprint_salt": "0123456789abcdef",
            "agent_dispatcher": {
                "max_attempts": 2,
                "max_session_ms": 60_000,
                "max_projection_events": 8
            },
            "instances": [{
                "alias": "node.a",
                "instance_id": id.transport(),
                "application_id": "neutral.application",
                "adb_path": "adb",
                "touch_backend": "maatouch",
                "capture_backend": "adb"
            }]
        });
        let config =
            serde_json::from_value::<ActingdConfigFile>(base.clone()).expect("typed config");
        config.assemble().expect("bounded dispatcher config");

        let mut invalid = base;
        invalid["agent_dispatcher"]["max_attempts"] = json!(0);
        let config = serde_json::from_value::<ActingdConfigFile>(invalid).expect("typed config");
        assert_eq!(
            config.assemble().err(),
            Some("agent_dispatcher_config_invalid")
        );
    }
}
