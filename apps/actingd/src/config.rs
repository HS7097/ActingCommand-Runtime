// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_contract::resource_declaration::{
    ProcedureBindingConfigFile, ScheduledExecutionConfigFile,
};
use actingcommand_contract::{
    ApplicationLifecycleAction, ContainedTaskRequest, EmulatorInstanceAction, InstanceId,
    RuntimeConfigManifest,
};
use actingcommand_device::{
    AdbConfig, CaptureBackend, CaptureBackendChoice, CaptureBackendConfig, CaptureBackendName,
    DeviceError, DeviceErrorCategory, DeviceErrorDiagnosticMessage, DeviceErrorSensitivity,
    DeviceResult, DeviceTarget, Frame, InputBackend, MaaTouchConfig, MinitouchConfig, PixelFormat,
    PreparedSegmentedSwipePlan, TouchBackendChoice, TouchBackendConfig,
};
use actingcommand_policy::{
    CatalogDocumentSource, CatalogSources, EvaluationFacts, EvaluationResources, MAX_APPROVAL_REFS,
    MAX_CATALOG_BYTES, MAX_DOCUMENT_BYTES, MAX_REFERENCES_PER_TASK, MAX_TASKS, compile_catalog,
};
use actingcommand_runtime_host::{
    AgentDispatcherConfig, EmulatorControlFailure, EmulatorControlOutcome, EmulatorControlResult,
    ExecutionBackendProvider, ExecutionBackendRegistration, ExecutionBackendRegistry,
    ForegroundApplicationObservation, PerformanceMonitorConfig, PolicyCadence, PolicyInputSnapshot,
    ProcedureBinding, ProcedureManifest, RecognitionVisionProvider, ResolvedExecutionInstance,
    RuntimeHostConfig, VisionFfiProvider, VisionModelIdentity,
};
use actingcommand_vision_ffi::{
    NnEngine, OcrEngine, VISION_PROVIDER_ARTIFACTS_SCHEMA_VERSION, VisionProviderArtifactManifest,
};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

mod manifest;
mod provider_startup;

const CONFIG_SCHEMA_VERSION: &str = "actingcommand.actingd.config.v1";
const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
const MAX_VISION_MANIFEST_BYTES: u64 = 1024 * 1024;
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
    /// The four defaulted scalars stay `Option` so the manifest can tell an explicit value
    /// from the library default; `assemble` applies the default.
    #[serde(default)]
    bind_port: Option<u16>,
    secret_fingerprint_salt: String,
    #[serde(default)]
    device_diagnostic_mode: Option<actingcommand_contract::DeviceDiagnosticMode>,
    #[serde(default)]
    capacity_thresholds: Option<actingcommand_contract::CapacityThresholds>,
    #[serde(default)]
    frame_retention_enabled: Option<bool>,
    #[serde(default)]
    governance_capability: Option<String>,
    #[serde(default)]
    agent_dispatcher: Option<AgentDispatcherConfigFile>,
    #[serde(default)]
    policy: Option<PolicyConfigFile>,
    #[serde(default)]
    vision_provider_manifest: Option<PathBuf>,
    /// Explicit MuMu install root: the highest-priority `MuMuManager.exe` discovery source.
    #[serde(default)]
    mumu_root: Option<PathBuf>,
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
struct InstanceConfig {
    alias: String,
    instance_id: InstanceId,
    #[serde(default)]
    application_id: Option<String>,
    /// Discovery binding key: the MuMu instance index reported by `MuMuManager info -v all`.
    #[serde(default)]
    instance_index: Option<u16>,
    /// Discovery binding key: the exact MuMu instance name. At most one key may be set.
    #[serde(default)]
    instance_name: Option<String>,
    #[serde(default)]
    nemu_app_index: Option<u32>,
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
    /// Slice #316-B3: the contained task the host runs by itself after a successful
    /// `emulator start` / `restart` of this instance; absent means nothing is pulled.
    #[serde(default)]
    startup_package: Option<StartupPackageConfigFile>,
    #[serde(default)]
    fixture_backend: Option<FixtureBackendConfigFile>,
}

/// Same semantics as `actingctl task-run --package <locator> --expected-sha256 <hex>`: the
/// locator (relative paths resolve against the configuration file's directory) and the
/// bare lowercase hex digest. The file is neither opened nor hashed at assembly.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StartupPackageConfigFile {
    package: PathBuf,
    expected_sha256: String,
}

impl StartupPackageConfigFile {
    fn request(self, source_root: &Path) -> Result<ContainedTaskRequest, &'static str> {
        let path = if self.package.is_absolute() {
            self.package
        } else {
            source_root.join(self.package)
        };
        if !path.is_absolute() {
            return Err("startup_package_path_invalid");
        }
        let digest = actingcommand_contract::PackageRef::from(self.expected_sha256);
        if !matches!(
            digest,
            actingcommand_contract::PackageRef::LegacyZipSha256(_)
        ) || digest.validate().is_err()
        {
            return Err("startup_package_digest_invalid");
        }
        ContainedTaskRequest::new(path.to_string_lossy().into_owned(), digest)
            .and_then(|request| {
                request.with_response_deadline_ms(ContainedTaskRequest::MAX_RESPONSE_DEADLINE_MS)
            })
            .map_err(|_| "startup_package_invalid")
    }
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
    /// The manifest handed to `host`; `check-config` prints it.
    pub(super) manifest: RuntimeConfigManifest,
}

pub(super) struct PolicyBootstrap {
    pub(super) state_root: PathBuf,
    pub(super) governance_capability: String,
    pub(super) catalog_approval_ids: Vec<String>,
    pub(super) catalog: CatalogSources,
    pub(super) scheduled_tasks: BTreeMap<String, ScheduledProcedureTask>,
    pub(super) registry_modes: BTreeMap<String, ScheduledExecutionMode>,
    pub(super) cadence: PolicyCadence,
}

pub(super) struct ConfiguredExecutionBackendRegistry {
    pending_vision: Option<(PathBuf, PathBuf)>,
    devices: Option<ExecutionBackendRegistry>,
    device_input_backends: BTreeMap<String, TouchBackendChoice>,
    device_capture_backends: BTreeMap<String, CaptureBackendChoice>,
    fixtures: Option<FixtureExecutionBackendRegistry>,
    modes: BTreeMap<String, ScheduledExecutionMode>,
    /// Instances bound by `instance_index`/`instance_name`; registered by provider startup
    /// after one `MuMuManager` discovery run. `assemble` itself spawns nothing.
    deferred: Vec<DeferredInstance>,
    mumu_root: Option<PathBuf>,
}

/// The discovery binding key of one deferred instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum InstanceBindingKey {
    Index(u16),
    Name(String),
}

impl InstanceBindingKey {
    pub(super) const fn index(&self) -> Option<u16> {
        match self {
            Self::Index(index) => Some(*index),
            Self::Name(_) => None,
        }
    }

    pub(super) fn name(&self) -> Option<&str> {
        match self {
            Self::Index(_) => None,
            Self::Name(name) => Some(name),
        }
    }
}

impl std::fmt::Display for InstanceBindingKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Index(index) => write!(formatter, "instance_index={index}"),
            Self::Name(name) => write!(formatter, "instance_name={name:?}"),
        }
    }
}

pub(super) struct DeferredInstance {
    alias: String,
    key: InstanceBindingKey,
    /// Validated declaration; its ADB target is completed from discovery at startup.
    config: InstanceConfig,
}

pub(super) struct FixtureExecutionBackendRegistry {
    instances: BTreeMap<String, FixtureExecutionBackend>,
    vision_provider: Option<Arc<dyn RecognitionVisionProvider>>,
}

struct FixtureExecutionBackend {
    instance_id: InstanceId,
    frames: Vec<Frame>,
    max_inputs: u16,
}

enum ConfiguredInstanceBackend {
    Device {
        alias: String,
        instance_id: InstanceId,
        input_backend: TouchBackendChoice,
        capture_backend: CaptureBackendChoice,
        registration: Box<ExecutionBackendRegistration>,
    },
    Fixture {
        alias: String,
        backend: FixtureExecutionBackend,
    },
    Deferred(Box<DeferredInstance>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ScheduledExecutionMode {
    FixtureSimulation,
    DeviceRegistry,
}

pub(super) struct ScheduledProcedureTask {
    pub(super) request: ContainedTaskRequest,
    pub(super) mode: ScheduledExecutionMode,
}

type ConfiguredScheduledProcedureTask = (String, ScheduledProcedureTask);
type AssembledProcedureBinding = (ProcedureBinding, Option<ConfiguredScheduledProcedureTask>);

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
    pub(super) fn maintenance_config(self) -> Result<RuntimeHostConfig, &'static str> {
        if self.schema_version != CONFIG_SCHEMA_VERSION
            || self.state_root.as_os_str().is_empty()
            || !(16..=1024).contains(&self.secret_fingerprint_salt.len())
        {
            return Err("maintenance_config_invalid");
        }
        Ok(RuntimeHostConfig::new(
            self.state_root,
            self.secret_fingerprint_salt.as_bytes(),
        ))
    }

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
        if self
            .mumu_root
            .as_ref()
            .is_some_and(|root| root.as_os_str().is_empty() || !root.is_absolute())
        {
            return Err("mumu_root_invalid");
        }
        let mut instances = self.instances;
        let mut startup_packages = BTreeMap::new();
        for instance in &mut instances {
            if let Some(startup_package) = instance.startup_package.take() {
                if instance.fixture_backend.is_some() {
                    return Err("instance_config_invalid");
                }
                startup_packages.insert(
                    instance.alias.clone(),
                    startup_package.request(&self.source_root)?,
                );
            }
        }
        let registrations = instances
            .into_iter()
            .map(InstanceConfig::backend)
            .collect::<Result<Vec<_>, _>>()?;
        let mut registry = ConfiguredExecutionBackendRegistry::new(registrations, None)?;
        registry.mumu_root = self.mumu_root;
        registry.pending_vision = self
            .vision_provider_manifest
            .map(|path| (self.source_root.clone(), path));
        let policy = self
            .policy
            .map(|policy| policy.assemble(&self.source_root))
            .transpose()?;
        if let Some(policy) = policy.as_ref() {
            policy.validate_registry_modes(&registry)?;
        }
        let policy_state_root = self.state_root.clone();
        let policy_governance_capability = self.governance_capability.clone();
        let policy_cadence = PolicyCadence::default();
        let agent_dispatcher_budget = self.agent_dispatcher.as_ref().map(|dispatcher| {
            (
                dispatcher.max_attempts,
                dispatcher.max_session_ms,
                dispatcher.max_projection_events,
            )
        });
        let mut host =
            RuntimeHostConfig::new(self.state_root, self.secret_fingerprint_salt.as_bytes())
                .with_device_diagnostic_mode(self.device_diagnostic_mode.unwrap_or_default())
                .with_capacity_thresholds(self.capacity_thresholds.unwrap_or_default())
                .with_frame_retention_enabled(self.frame_retention_enabled.unwrap_or_default())
                .with_bind_address(SocketAddr::new(
                    bind_host,
                    self.bind_port.unwrap_or_default(),
                ))
                .with_policy_cadence(policy_cadence.clone())
                .with_performance_monitor(PerformanceMonitorConfig::default());
        let instances_startup_package_count = startup_packages.len();
        host = host.with_startup_packages(startup_packages);
        let manifest = manifest::build(&manifest::ManifestInputs {
            bind_host,
            bind_port: self.bind_port,
            device_diagnostic_mode: self.device_diagnostic_mode,
            frame_retention_enabled: self.frame_retention_enabled,
            capacity_thresholds: self.capacity_thresholds,
            secret_fingerprint_salt_bytes: self.secret_fingerprint_salt.len(),
            mumu_root: registry.mumu_root.as_deref(),
            governance_configured: self.governance_capability.is_some(),
            agent_dispatcher: agent_dispatcher_budget,
            policy_configured: policy.is_some(),
            vision_provider_configured: registry.pending_vision.is_some(),
            instances_count: registry.modes.len(),
            instances_deferred_count: registry.deferred.len(),
            instances_startup_package_count,
            policy_cadence: &policy_cadence,
            io_timeout: host.io_timeout(),
            maximum_frame_bytes: host.maximum_frame_bytes(),
        })?;
        host = host.with_config_manifest(manifest.clone());
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
                registry_modes: registry.modes.clone(),
                cadence: policy_cadence,
            })
        } else {
            None
        };
        Ok(RuntimeAssembly {
            host,
            registry,
            policy,
            manifest,
        })
    }
}

struct PolicyAssembly {
    inputs: PolicyInputSnapshot,
    procedure_manifest: ProcedureManifest,
    catalog_approval_ids: Vec<String>,
    catalog: CatalogSources,
    scheduled_tasks: BTreeMap<String, ScheduledProcedureTask>,
    scheduled_instance_scopes: Vec<(String, String)>,
}

impl PolicyConfigFile {
    fn assemble(self, source_root: &Path) -> Result<PolicyAssembly, &'static str> {
        if self.procedure_manifest.is_empty() || self.procedure_manifest.len() > MAX_TASKS {
            return Err("procedure_manifest_size_invalid");
        }
        let mut bindings = Vec::with_capacity(self.procedure_manifest.len());
        let mut scheduled_tasks = BTreeMap::new();
        for configured in self.procedure_manifest {
            let (binding, scheduled_task) = procedure_binding(configured, source_root)?;
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
        let scheduled_instance_scopes = compiled
            .catalog()
            .tasks
            .tasks
            .iter()
            .filter_map(|task| {
                scheduled_tasks
                    .contains_key(&task.procedure_ref)
                    .then_some((&task.procedure_ref, &task.scope))
            })
            .filter_map(|(procedure_ref, scope)| match scope {
                actingcommand_policy::ScopeSelector::Instance { instance_id } => {
                    Some((procedure_ref.clone(), instance_id.clone()))
                }
                actingcommand_policy::ScopeSelector::Server { .. }
                | actingcommand_policy::ScopeSelector::Game { .. } => None,
            })
            .collect();
        Ok(PolicyAssembly {
            inputs: PolicyInputSnapshot::new(self.facts, self.resources),
            procedure_manifest,
            catalog_approval_ids: self.catalog_approval_ids,
            catalog,
            scheduled_tasks,
            scheduled_instance_scopes,
        })
    }
}

impl PolicyAssembly {
    fn validate_registry_modes(
        &self,
        registry: &ConfiguredExecutionBackendRegistry,
    ) -> Result<(), &'static str> {
        for (procedure_ref, instance_alias) in &self.scheduled_instance_scopes {
            let scheduled = self
                .scheduled_tasks
                .get(procedure_ref)
                .ok_or("scheduled_execution_binding_missing")?;
            let actual = registry
                .mode_for_alias(instance_alias)
                .ok_or("scheduled_execution_instance_unknown")?;
            if actual != scheduled.mode {
                return Err("scheduled_execution_backend_mode_mismatch");
            }
        }
        for scheduled in self.scheduled_tasks.values() {
            if !registry.modes.values().any(|mode| mode == &scheduled.mode) {
                return Err("scheduled_execution_backend_mode_unavailable");
            }
        }
        Ok(())
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

fn procedure_binding(
    configured: ProcedureBindingConfigFile,
    source_root: &Path,
) -> Result<AssembledProcedureBinding, &'static str> {
    let ProcedureBindingConfigFile {
        procedure_ref,
        package_digest,
        operation_id,
        yield_points,
        scheduled_execution,
    } = configured;
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
        Some(ScheduledExecutionConfigFile::FixtureSimulation { package_path }) => Some((
            procedure_ref,
            ScheduledProcedureTask {
                request: contained_task_request(source_root, &package_digest, package_path)?,
                mode: ScheduledExecutionMode::FixtureSimulation,
            },
        )),
        Some(ScheduledExecutionConfigFile::DeviceRegistry { package_path }) => Some((
            procedure_ref,
            ScheduledProcedureTask {
                request: contained_task_request(source_root, &package_digest, package_path)?,
                mode: ScheduledExecutionMode::DeviceRegistry,
            },
        )),
    };
    Ok((binding, scheduled_task))
}

fn contained_task_request(
    source_root: &Path,
    package_digest: &actingcommand_contract::PackageRef,
    package_path: Option<PathBuf>,
) -> Result<ContainedTaskRequest, &'static str> {
    let package_path = package_path.ok_or("procedure_package_path_missing")?;
    let path = if package_path.is_absolute() {
        package_path
    } else {
        source_root.join(package_path)
    };
    // Preserve source locator components so containment can reject link ambiguity.
    let path = match package_digest {
        actingcommand_contract::PackageRef::LegacyZipSha256(_) => {
            fs::canonicalize(path).map_err(|_| "procedure_package_unavailable")?
        }
        actingcommand_contract::PackageRef::GitSourceTree(_) if path.is_absolute() => path,
        actingcommand_contract::PackageRef::GitSourceTree(_) => std::env::current_dir()
            .map_err(|_| "procedure_package_unavailable")?
            .join(path),
    };
    let metadata = fs::metadata(&path).map_err(|_| "procedure_package_unavailable")?;
    if match package_digest {
        actingcommand_contract::PackageRef::LegacyZipSha256(_) => !metadata.is_file(),
        actingcommand_contract::PackageRef::GitSourceTree(_) => !metadata.is_dir(),
    } {
        return Err("procedure_package_not_regular");
    }
    package_digest
        .validate()
        .map_err(|_| "procedure_package_digest_invalid")?;
    ContainedTaskRequest::new(path.to_string_lossy().into_owned(), package_digest.clone())
        .and_then(|request| {
            request.with_response_deadline_ms(ContainedTaskRequest::MAX_RESPONSE_DEADLINE_MS)
        })
        .map_err(|_| "procedure_task_request_invalid")
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
        } else if let Some(key) = self.binding_key()? {
            self.deferred_backend(key)
        } else {
            self.device_backend()
        }
    }

    fn binding_key(&self) -> Result<Option<InstanceBindingKey>, &'static str> {
        match (self.instance_index, self.instance_name.as_deref()) {
            (None, None) => Ok(None),
            (Some(_), Some(_)) => Err("instance_binding_key_invalid"),
            (Some(index), None) => Ok(Some(InstanceBindingKey::Index(index))),
            (None, Some(name)) => {
                if name.trim().is_empty()
                    || name.len() > actingcommand_device::MAX_MUMU_INSTANCE_NAME_BYTES
                    || name.chars().any(char::is_control)
                {
                    return Err("instance_binding_key_invalid");
                }
                Ok(Some(InstanceBindingKey::Name(name.to_owned())))
            }
        }
    }

    /// Validates everything that does not need discovery; the ADB target is completed later.
    /// Declared `adb_path`/`host`/`port` stay declared values to be cross-checked; no default
    /// host or port applies to a discovery-bound instance.
    fn deferred_backend(
        self,
        key: InstanceBindingKey,
    ) -> Result<ConfiguredInstanceBackend, &'static str> {
        if self.serial.is_some() {
            return Err("instance_binding_key_invalid");
        }
        if self
            .adb_path
            .as_ref()
            .is_some_and(|value| value.trim().is_empty())
            || self
                .host
                .as_ref()
                .is_some_and(|value| value.trim().is_empty())
            || self.port == Some(0)
        {
            return Err("instance_config_invalid");
        }
        self.application_id
            .as_ref()
            .filter(|value| !value.trim().is_empty())
            .ok_or("application_identity_missing")?;
        self.backend_choices()?;
        for timeout in [
            self.command_timeout_ms,
            self.handshake_timeout_ms,
            self.shutdown_timeout_ms,
            self.tap_hold_ms,
        ] {
            bounded_duration(timeout)?;
        }
        Ok(ConfiguredInstanceBackend::Deferred(Box::new(
            DeferredInstance {
                alias: self.alias.clone(),
                key,
                config: self,
            },
        )))
    }

    fn backend_choices(&self) -> Result<(TouchBackendChoice, CaptureBackendChoice), &'static str> {
        let touch_backend = self
            .touch_backend
            .as_deref()
            .ok_or("touch_backend_invalid")?;
        let capture_backend = self
            .capture_backend
            .as_deref()
            .ok_or("capture_backend_invalid")?;
        let requested =
            TouchBackendChoice::parse(touch_backend).map_err(|_| "touch_backend_invalid")?;
        if matches!(
            requested,
            TouchBackendChoice::Auto | TouchBackendChoice::AutoFastest
        ) {
            return Err("touch_backend_must_be_explicit");
        }
        let capture_requested =
            CaptureBackendChoice::parse(capture_backend).map_err(|_| "capture_backend_invalid")?;
        if matches!(
            capture_requested,
            CaptureBackendChoice::Auto | CaptureBackendChoice::AutoFastest
        ) {
            return Err("capture_backend_must_be_explicit");
        }
        Ok((requested, capture_requested))
    }

    fn device_backend(self) -> Result<ConfiguredInstanceBackend, &'static str> {
        let adb_path = self.adb_path.clone().ok_or("instance_config_invalid")?;
        let host = self.host.clone().unwrap_or_else(default_device_host);
        let port = self.port.unwrap_or_else(default_device_port);
        self.device_registration(adb_path, host, Some(port))
    }

    /// Builds the device registration for one ADB target (explicit or discovered). `None` is
    /// a discovered instance that is stopped: it registers with a pending endpoint and the
    /// port placeholder is never dispatched (`ExecutionBackendRegistry` refuses every session
    /// while pending).
    fn device_registration(
        self,
        adb_path: String,
        host: String,
        port: Option<u16>,
    ) -> Result<ConfiguredInstanceBackend, &'static str> {
        let connect = self.connect.unwrap_or_else(enabled);
        if adb_path.trim().is_empty()
            || host.trim().is_empty()
            || port == Some(0)
            || self
                .serial
                .as_ref()
                .is_some_and(|value| value.trim().is_empty())
        {
            return Err("instance_config_invalid");
        }
        let (requested, capture_requested) = self.backend_choices()?;
        let nemu_app_index = match (requested, self.nemu_app_index) {
            (TouchBackendChoice::NemuIpc, Some(index))
                if capture_requested == CaptureBackendChoice::NemuIpc =>
            {
                Some(
                    actingcommand_device::NemuAppIndex::try_from(index)
                        .map_err(|_| "nemu_app_index_invalid")?,
                )
            }
            (TouchBackendChoice::NemuIpc, _) => {
                return Err("nemu_paired_input_configuration_missing");
            }
            (_, Some(_)) => return Err("nemu_app_index_requires_paired_input"),
            (_, None) => None,
        };
        let application_id = self
            .application_id
            .filter(|value| !value.trim().is_empty())
            .ok_or("application_identity_missing")?;
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
            port: port.unwrap_or(0),
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
        let alias = self.alias;
        let instance_id = self.instance_id;
        ExecutionBackendRegistration::new(
            alias.clone(),
            instance_id,
            application_id,
            touch,
            capture,
        )
        .and_then(|registration| match nemu_app_index {
            Some(index) => registration.with_nemu_app_index(index),
            None => Ok(registration),
        })
        .map(Box::new)
        .map(|registration| ConfiguredInstanceBackend::Device {
            alias,
            instance_id,
            input_backend: requested,
            capture_backend: capture_requested,
            registration,
        })
        .map_err(|_| "instance_registration_invalid")
    }

    fn fixture_backend(self) -> Result<ConfiguredInstanceBackend, &'static str> {
        if self.application_id.is_some()
            || self.startup_package.is_some()
            || self.nemu_app_index.is_some()
            || self.instance_index.is_some()
            || self.instance_name.is_some()
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
        if actingcommand_contract::validate_instance_alias(&self.alias).is_err()
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
    fn new(
        backends: Vec<ConfiguredInstanceBackend>,
        vision_provider: Option<Arc<dyn RecognitionVisionProvider>>,
    ) -> Result<Self, &'static str> {
        let mut devices = Vec::new();
        let mut device_input_backends = BTreeMap::new();
        let mut device_capture_backends = BTreeMap::new();
        let mut fixtures = BTreeMap::new();
        let mut modes = BTreeMap::new();
        let mut instance_ids = BTreeSet::new();
        let mut deferred = Vec::new();
        for backend in backends {
            match backend {
                ConfiguredInstanceBackend::Device {
                    alias,
                    instance_id,
                    input_backend,
                    capture_backend,
                    registration,
                } => {
                    if modes
                        .insert(alias.clone(), ScheduledExecutionMode::DeviceRegistry)
                        .is_some()
                        || device_input_backends
                            .insert(alias.clone(), input_backend)
                            .is_some()
                        || device_capture_backends
                            .insert(alias, capture_backend)
                            .is_some()
                        || !instance_ids.insert(instance_id)
                    {
                        return Err("execution_registry_invalid");
                    }
                    devices.push(*registration);
                }
                ConfiguredInstanceBackend::Fixture { alias, backend } => {
                    if modes
                        .insert(alias.clone(), ScheduledExecutionMode::FixtureSimulation)
                        .is_some()
                        || !instance_ids.insert(backend.instance_id)
                        || fixtures.insert(alias, backend).is_some()
                    {
                        return Err("execution_registry_invalid");
                    }
                }
                ConfiguredInstanceBackend::Deferred(entry) => {
                    if modes
                        .insert(entry.alias.clone(), ScheduledExecutionMode::DeviceRegistry)
                        .is_some()
                        || !instance_ids.insert(entry.config.instance_id)
                    {
                        return Err("execution_registry_invalid");
                    }
                    deferred.push(*entry);
                }
            }
        }
        let devices = (!devices.is_empty())
            .then(|| ExecutionBackendRegistry::new(devices))
            .transpose()
            .map_err(|_| "execution_registry_invalid")?
            .map(|registry| match &vision_provider {
                Some(provider) => registry.with_vision_provider(Arc::clone(provider)),
                None => registry,
            });
        let fixtures = (!fixtures.is_empty()).then_some(FixtureExecutionBackendRegistry {
            instances: fixtures,
            vision_provider,
        });
        Ok(Self {
            pending_vision: None,
            devices,
            device_input_backends,
            device_capture_backends,
            fixtures,
            modes,
            deferred,
            mumu_root: None,
        })
    }

    pub(super) fn mode_for_alias(&self, instance_alias: &str) -> Option<ScheduledExecutionMode> {
        self.modes.get(instance_alias).copied()
    }

    /// The binding key of an instance still waiting for discovery (`check-config` reporting).
    pub(super) fn deferred_binding(&self, instance_alias: &str) -> Option<&InstanceBindingKey> {
        self.deferred
            .iter()
            .find(|entry| entry.alias == instance_alias)
            .map(|entry| &entry.key)
    }

    /// Registers one resolved device entry under the same duplicate rules as `new`.
    fn register_device(&mut self, backend: ConfiguredInstanceBackend) -> Result<(), &'static str> {
        let ConfiguredInstanceBackend::Device {
            alias,
            input_backend,
            capture_backend,
            registration,
            ..
        } = backend
        else {
            return Err("execution_registry_invalid");
        };
        if self.mode_for_alias(&alias) != Some(ScheduledExecutionMode::DeviceRegistry)
            || self
                .device_input_backends
                .insert(alias.clone(), input_backend)
                .is_some()
            || self
                .device_capture_backends
                .insert(alias, capture_backend)
                .is_some()
        {
            return Err("execution_registry_invalid");
        }
        match self.devices.as_mut() {
            Some(devices) => devices.register(*registration),
            None => ExecutionBackendRegistry::new([*registration]).map(|devices| {
                self.devices = Some(devices);
            }),
        }
        .map_err(|_| "execution_registry_invalid")
    }
}

impl ExecutionBackendProvider for ConfiguredExecutionBackendRegistry {
    fn open_nemu_session(
        &self,
        instance_alias: &str,
    ) -> DeviceResult<Option<actingcommand_device::NemuSessionBackends>> {
        match self.mode_for_alias(instance_alias) {
            Some(ScheduledExecutionMode::DeviceRegistry) => {
                let selected = self.device_input_backends.get(instance_alias).copied();
                if selected != Some(TouchBackendChoice::NemuIpc) {
                    return Ok(None);
                }
                open_device_registry_input_with_diagnostic(selected, || {
                    let mut pair = self
                        .devices
                        .as_ref()
                        .ok_or_else(|| DeviceError::fatal("device registry is unavailable"))?
                        .open_nemu_session(instance_alias)?
                        .ok_or_else(|| {
                            DeviceError::fatal("selected Nemu paired session is unavailable")
                        })?;
                    pair.input = Box::new(DeviceRegistryInputDiagnosticBackend::new(
                        pair.input,
                        TouchBackendChoice::NemuIpc,
                    ));
                    Ok(Some(pair))
                })
            }
            Some(ScheduledExecutionMode::FixtureSimulation) => Ok(None),
            None => Err(DeviceError::fatal(
                "execution backend instance is not registered",
            )),
        }
    }
    fn instance_aliases(&self) -> Vec<String> {
        self.modes.keys().cloned().collect()
    }

    fn resolve(&self, instance_alias: &str) -> Option<ResolvedExecutionInstance> {
        match self.mode_for_alias(instance_alias)? {
            ScheduledExecutionMode::DeviceRegistry => {
                self.devices.as_ref()?.resolve(instance_alias)
            }
            ScheduledExecutionMode::FixtureSimulation => {
                self.fixtures.as_ref()?.resolve(instance_alias)
            }
        }
    }

    fn open_input(&self, instance_alias: &str) -> DeviceResult<Box<dyn InputBackend>> {
        match self.mode_for_alias(instance_alias) {
            Some(ScheduledExecutionMode::DeviceRegistry) => {
                let input_backend = self.device_input_backends.get(instance_alias).copied();
                open_device_registry_input_with_diagnostic(input_backend, || {
                    let input_backend = input_backend.ok_or_else(|| {
                        DeviceError::fatal("device input backend context is unavailable")
                    })?;
                    let backend = self
                        .devices
                        .as_ref()
                        .ok_or_else(|| DeviceError::fatal("device registry is unavailable"))?
                        .open_input(instance_alias)?;
                    Ok(Box::new(DeviceRegistryInputDiagnosticBackend::new(
                        backend,
                        input_backend,
                    )) as Box<dyn InputBackend>)
                })
            }
            Some(ScheduledExecutionMode::FixtureSimulation) => self
                .fixtures
                .as_ref()
                .ok_or_else(|| DeviceError::fatal("fixture registry is unavailable"))?
                .open_input(instance_alias),
            None => Err(DeviceError::fatal(
                "execution backend instance is not registered",
            )),
        }
    }

    fn open_capture(&self, instance_alias: &str) -> DeviceResult<Box<dyn CaptureBackend>> {
        match self.mode_for_alias(instance_alias) {
            Some(ScheduledExecutionMode::DeviceRegistry) => {
                let capture_backend = self.device_capture_backends.get(instance_alias).copied();
                open_device_registry_capture_with_diagnostic(capture_backend, || {
                    capture_backend.ok_or_else(|| {
                        DeviceError::fatal("device capture backend context is unavailable")
                    })?;
                    self.devices
                        .as_ref()
                        .ok_or_else(|| DeviceError::fatal("device registry is unavailable"))?
                        .open_capture(instance_alias)
                })
            }
            Some(ScheduledExecutionMode::FixtureSimulation) => self
                .fixtures
                .as_ref()
                .ok_or_else(|| DeviceError::fatal("fixture registry is unavailable"))?
                .open_capture(instance_alias),
            None => Err(DeviceError::fatal(
                "execution backend instance is not registered",
            )),
        }
    }

    fn control_application(
        &self,
        instance_alias: &str,
        action: ApplicationLifecycleAction,
    ) -> DeviceResult<()> {
        match self.mode_for_alias(instance_alias) {
            Some(ScheduledExecutionMode::DeviceRegistry) => self
                .devices
                .as_ref()
                .ok_or_else(|| DeviceError::fatal("device registry is unavailable"))?
                .control_application(instance_alias, action),
            Some(ScheduledExecutionMode::FixtureSimulation) => self
                .fixtures
                .as_ref()
                .ok_or_else(|| DeviceError::fatal("fixture registry is unavailable"))?
                .control_application(instance_alias, action),
            None => Err(DeviceError::fatal(
                "execution backend instance is not registered",
            )),
        }
    }

    fn probe_adb_baseline(&self, instance_alias: &str) -> DeviceResult<()> {
        match self.mode_for_alias(instance_alias) {
            Some(ScheduledExecutionMode::DeviceRegistry) => self
                .devices
                .as_ref()
                .ok_or_else(|| DeviceError::fatal("device registry is unavailable"))?
                .probe_adb_baseline(instance_alias),
            // A fixture has no ADB baseline to wait for.
            Some(ScheduledExecutionMode::FixtureSimulation) => Ok(()),
            None => Err(DeviceError::fatal(
                "execution backend instance is not registered",
            )),
        }
    }

    fn observe_foreground_application(
        &self,
        instance_alias: &str,
    ) -> DeviceResult<ForegroundApplicationObservation> {
        match self.mode_for_alias(instance_alias) {
            Some(ScheduledExecutionMode::DeviceRegistry) => self
                .devices
                .as_ref()
                .ok_or_else(|| DeviceError::fatal("device registry is unavailable"))?
                .observe_foreground_application(instance_alias),
            // A fixture has no ADB baseline; the host never asks for one.
            Some(ScheduledExecutionMode::FixtureSimulation) => self
                .fixtures
                .as_ref()
                .ok_or_else(|| DeviceError::fatal("fixture registry is unavailable"))?
                .observe_foreground_application(instance_alias),
            None => Err(DeviceError::fatal(
                "execution backend instance is not registered",
            )),
        }
    }

    fn control_instance(
        &self,
        instance_alias: &str,
        action: EmulatorInstanceAction,
    ) -> EmulatorControlResult<EmulatorControlOutcome> {
        let refused = |message: &str, stage: &'static str| {
            EmulatorControlFailure::without_output(
                DeviceError::fatal(message)
                    .with_diagnostic(DeviceErrorCategory::Protocol, stage)
                    .with_diagnostic_context(
                        "configured_execution_backend_registry",
                        "control_instance",
                        DeviceErrorSensitivity::Sensitive,
                    ),
                0,
            )
        };
        match self.mode_for_alias(instance_alias) {
            Some(ScheduledExecutionMode::DeviceRegistry) => self
                .devices
                .as_ref()
                .ok_or_else(|| {
                    refused(
                        "device registry is unavailable",
                        "emulator_control.unavailable",
                    )
                })?
                .control_instance(instance_alias, action),
            Some(ScheduledExecutionMode::FixtureSimulation) => Err(refused(
                "emulator control unavailable: a fixture simulation instance has no emulator",
                "emulator_control.unavailable",
            )),
            None => Err(refused(
                "execution backend instance is not registered",
                "emulator_control.unregistered",
            )),
        }
    }

    fn rebind_discovered_endpoint(
        &self,
        instance_alias: &str,
        adb_port: Option<u16>,
    ) -> DeviceResult<()> {
        let refused = |message: &str, stage: &'static str| {
            DeviceError::fatal(message)
                .with_diagnostic(DeviceErrorCategory::Protocol, stage)
                .with_diagnostic_context(
                    "configured_execution_backend_registry",
                    "rebind_discovered_endpoint",
                    DeviceErrorSensitivity::Sensitive,
                )
        };
        match self.mode_for_alias(instance_alias) {
            Some(ScheduledExecutionMode::DeviceRegistry) => self
                .devices
                .as_ref()
                .ok_or_else(|| {
                    refused(
                        "device registry is unavailable",
                        "emulator_control.unavailable",
                    )
                })?
                .rebind_discovered_endpoint(instance_alias, adb_port),
            Some(ScheduledExecutionMode::FixtureSimulation) => Err(refused(
                "endpoint rebinding unavailable: a fixture simulation instance has no emulator",
                "emulator_control.unavailable",
            )),
            None => Err(refused(
                "execution backend instance is not registered",
                "emulator_control.unregistered",
            )),
        }
    }

    fn vision_provider(&self) -> Option<Arc<dyn RecognitionVisionProvider>> {
        self.devices
            .as_ref()
            .and_then(ExecutionBackendProvider::vision_provider)
            .or_else(|| {
                self.fixtures
                    .as_ref()
                    .and_then(ExecutionBackendProvider::vision_provider)
            })
    }
}

struct DeviceRegistryInputDiagnosticBackend {
    backend: Box<dyn InputBackend>,
    requested_backend: TouchBackendChoice,
}

impl DeviceRegistryInputDiagnosticBackend {
    fn new(backend: Box<dyn InputBackend>, requested_backend: TouchBackendChoice) -> Self {
        Self {
            backend,
            requested_backend,
        }
    }

    fn run<T>(
        &mut self,
        operation: &'static str,
        execute: impl FnOnce(&mut dyn InputBackend) -> DeviceResult<T>,
    ) -> DeviceResult<T> {
        match execute(self.backend.as_mut()) {
            Ok(value) => Ok(value),
            Err(error) => {
                let producer_complete =
                    error.diagnostic().is_some() && error.diagnostic_context().is_some();
                let error = error
                    .with_diagnostic_if_absent(
                        DeviceErrorCategory::Native,
                        "device_registry.input.operation",
                    )
                    .with_diagnostic_context_if_absent(
                        self.requested_backend.as_str(),
                        operation,
                        DeviceErrorSensitivity::Sensitive,
                    );
                let error = if producer_complete {
                    error
                } else {
                    error.with_diagnostic_message(
                        DeviceErrorDiagnosticMessage::DeviceRegistryInputOperationFailed,
                    )
                };
                Err(error)
            }
        }
    }
}

impl InputBackend for DeviceRegistryInputDiagnosticBackend {
    fn selection_context(&self) -> Option<actingcommand_device::InputSelectionContext> {
        self.backend.selection_context()
    }

    fn tap(&mut self, x: i32, y: i32) -> DeviceResult<()> {
        self.run("tap", |backend| backend.tap(x, y))
    }

    fn tap_in_frame(
        &mut self,
        x: i32,
        y: i32,
        context: &actingcommand_device::InputExecutionContext,
    ) -> DeviceResult<()> {
        self.run("tap", |backend| backend.tap_in_frame(x, y, context))
    }

    fn long_tap(&mut self, x: i32, y: i32, duration_ms: u64) -> DeviceResult<()> {
        self.run("long_tap", |backend| backend.long_tap(x, y, duration_ms))
    }

    fn swipe(&mut self, x1: i32, y1: i32, x2: i32, y2: i32, duration_ms: u64) -> DeviceResult<()> {
        self.run("swipe", |backend| {
            backend.swipe(x1, y1, x2, y2, duration_ms)
        })
    }

    fn supports_segmented_swipe(&self) -> bool {
        self.backend.supports_segmented_swipe()
    }

    fn segmented_swipe_prepared(&mut self, plan: &PreparedSegmentedSwipePlan) -> DeviceResult<()> {
        self.run("segmented_swipe", |backend| {
            backend.segmented_swipe_prepared(plan)
        })
    }

    fn segmented_swipe_prepared_in_frame(
        &mut self,
        plan: &PreparedSegmentedSwipePlan,
        context: &actingcommand_device::InputExecutionContext,
    ) -> DeviceResult<()> {
        self.run("segmented_swipe", |backend| {
            backend.segmented_swipe_prepared_in_frame(plan, context)
        })
    }

    fn key(&mut self, key: &str) -> DeviceResult<()> {
        self.run("key", |backend| backend.key(key))
    }

    fn text(&mut self, text: &str) -> DeviceResult<()> {
        self.run("text", |backend| backend.text(text))
    }

    fn reset(&mut self) -> DeviceResult<()> {
        self.run("reset", |backend| backend.reset())
    }

    fn close_once(
        &mut self,
        authority: actingcommand_device::DeviceCloseAuthority,
    ) -> DeviceResult<actingcommand_device::DeviceResourceCloseOutcome> {
        self.run("close", |backend| backend.close_once(authority))
    }
}

fn open_device_registry_input_with_diagnostic<T>(
    input_backend: Option<TouchBackendChoice>,
    open: impl FnOnce() -> DeviceResult<T>,
) -> DeviceResult<T> {
    match open() {
        Ok(value) => Ok(value),
        Err(error) => {
            let producer_complete =
                error.diagnostic().is_some() && error.diagnostic_context().is_some();
            let error = error
                .with_diagnostic_if_absent(
                    DeviceErrorCategory::Native,
                    "device_registry.input.open",
                )
                .with_diagnostic_context_if_absent(
                    input_backend
                        .map(TouchBackendChoice::as_str)
                        .unwrap_or("unavailable"),
                    "open_input",
                    DeviceErrorSensitivity::Sensitive,
                );
            let error = if producer_complete {
                error
            } else {
                error.with_diagnostic_message(
                    DeviceErrorDiagnosticMessage::DeviceRegistryInputOpenFailed,
                )
            };
            Err(error)
        }
    }
}

fn open_device_registry_capture_with_diagnostic<T>(
    capture_backend: Option<CaptureBackendChoice>,
    open: impl FnOnce() -> DeviceResult<T>,
) -> DeviceResult<T> {
    match open() {
        Ok(value) => Ok(value),
        Err(error) => {
            let producer_complete =
                error.diagnostic().is_some() && error.diagnostic_context().is_some();
            let producer_message = error.diagnostic_message().is_some();
            let error = error
                .with_diagnostic_if_absent(
                    DeviceErrorCategory::Native,
                    "device_registry.capture.open",
                )
                .with_diagnostic_context_if_absent(
                    capture_backend
                        .map(CaptureBackendChoice::as_str)
                        .unwrap_or("unavailable"),
                    "open_capture",
                    DeviceErrorSensitivity::Sensitive,
                );
            let error = if producer_complete || producer_message {
                error
            } else {
                error.with_diagnostic_message(
                    DeviceErrorDiagnosticMessage::DeviceRegistryCaptureOpenFailed,
                )
            };
            Err(error)
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

    fn vision_provider(&self) -> Option<Arc<dyn RecognitionVisionProvider>> {
        self.vision_provider.clone()
    }
}

fn resolve_vision_artifact_paths(
    manifest: &mut VisionProviderArtifactManifest,
    artifact_root: &Path,
) {
    if let Some(artifacts) = &mut manifest.fastdeploy_ppocr {
        resolve_relative_path(artifact_root, &mut artifacts.provider_library_path);
        for path in &mut artifacts.runtime_library_paths {
            resolve_relative_path(artifact_root, path);
        }
        if let Some(path) = &mut artifacts.runtime_library_path {
            resolve_relative_path(artifact_root, path);
        }
        resolve_relative_path(artifact_root, &mut artifacts.detector_model_path);
        resolve_relative_path(artifact_root, &mut artifacts.recognizer_model_path);
        resolve_relative_path(artifact_root, &mut artifacts.dictionary_path);
        if let Some(path) = &mut artifacts.classifier_model_path {
            resolve_relative_path(artifact_root, path);
        }
    }
    if let Some(artifacts) = &mut manifest.onnxruntime {
        resolve_relative_path(artifact_root, &mut artifacts.provider_library_path);
        if let Some(path) = &mut artifacts.runtime_library_path {
            resolve_relative_path(artifact_root, path);
        }
        resolve_relative_path(artifact_root, &mut artifacts.model_path);
        if let Some(path) = &mut artifacts.labels_path {
            resolve_relative_path(artifact_root, path);
        }
    }
}

fn resolve_relative_path(root: &Path, path: &mut PathBuf) {
    if path.is_relative() {
        *path = root.join(&*path);
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

    fn close_once(
        &mut self,
        _authority: actingcommand_device::DeviceCloseAuthority,
    ) -> DeviceResult<actingcommand_device::DeviceResourceCloseOutcome> {
        Ok(actingcommand_device::DeviceResourceCloseOutcome::confirmed(
            0,
        ))
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

    fn close_once(
        &mut self,
        _authority: actingcommand_device::DeviceCloseAuthority,
    ) -> DeviceResult<actingcommand_device::DeviceResourceCloseOutcome> {
        let resource_count = u16::from(!self.closed);
        self.closed = true;
        Ok(actingcommand_device::DeviceResourceCloseOutcome::confirmed(
            resource_count,
        ))
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
    use actingcommand_device::{DeviceErrorCategory, SegmentedSwipeAction};
    use actingcommand_vision_ffi::{FastDeployPpocrArtifacts, OnnxExecutionProvider};
    use serde_json::json;
    use std::sync::Mutex;
    use tempfile::TempDir;

    #[derive(Debug, Clone, Copy)]
    enum TestInputOperation {
        Tap,
        LongTap,
        Swipe,
        Key,
        Text,
        Reset,
        Close,
    }

    const TEST_INPUT_OPERATIONS: [TestInputOperation; 7] = [
        TestInputOperation::Tap,
        TestInputOperation::LongTap,
        TestInputOperation::Swipe,
        TestInputOperation::Key,
        TestInputOperation::Text,
        TestInputOperation::Reset,
        TestInputOperation::Close,
    ];

    impl TestInputOperation {
        const fn name(self) -> &'static str {
            match self {
                Self::Tap => "tap",
                Self::LongTap => "long_tap",
                Self::Swipe => "swipe",
                Self::Key => "key",
                Self::Text => "text",
                Self::Reset => "reset",
                Self::Close => "close",
            }
        }

        fn invoke(self, backend: &mut dyn InputBackend) -> DeviceResult<()> {
            match self {
                Self::Tap => backend.tap(10, 20),
                Self::LongTap => backend.long_tap(10, 20, 30),
                Self::Swipe => backend.swipe(10, 20, 30, 40, 50),
                Self::Key => backend.key("KEYCODE_HOME"),
                Self::Text => backend.text("neutral fixture"),
                Self::Reset => backend.reset(),
                Self::Close => backend.close(),
            }
        }
    }

    struct RecordingInputBackend {
        result: DeviceResult<()>,
        calls: Arc<Mutex<Vec<&'static str>>>,
        segmented_swipe_actions: Arc<Mutex<Vec<SegmentedSwipeAction>>>,
        supports_segmented_swipe: bool,
    }

    impl RecordingInputBackend {
        fn invoke(&self, operation: &'static str) -> DeviceResult<()> {
            self.calls.lock().expect("input calls").push(operation);
            self.result.clone()
        }
    }

    impl InputBackend for RecordingInputBackend {
        fn tap(&mut self, _x: i32, _y: i32) -> DeviceResult<()> {
            self.invoke("tap")
        }

        fn long_tap(&mut self, _x: i32, _y: i32, _duration_ms: u64) -> DeviceResult<()> {
            self.invoke("long_tap")
        }

        fn swipe(
            &mut self,
            _x1: i32,
            _y1: i32,
            _x2: i32,
            _y2: i32,
            _duration_ms: u64,
        ) -> DeviceResult<()> {
            self.invoke("swipe")
        }

        fn supports_segmented_swipe(&self) -> bool {
            self.supports_segmented_swipe
        }

        fn segmented_swipe_prepared(
            &mut self,
            plan: &PreparedSegmentedSwipePlan,
        ) -> DeviceResult<()> {
            self.segmented_swipe_actions
                .lock()
                .expect("segmented swipe actions")
                .push(plan.action());
            self.invoke("segmented_swipe")
        }

        fn key(&mut self, _key: &str) -> DeviceResult<()> {
            self.invoke("key")
        }

        fn text(&mut self, _text: &str) -> DeviceResult<()> {
            self.invoke("text")
        }

        fn reset(&mut self) -> DeviceResult<()> {
            self.invoke("reset")
        }

        fn close_once(
            &mut self,
            _authority: actingcommand_device::DeviceCloseAuthority,
        ) -> DeviceResult<actingcommand_device::DeviceResourceCloseOutcome> {
            self.invoke("close")?;
            Ok(actingcommand_device::DeviceResourceCloseOutcome::confirmed(
                1,
            ))
        }
    }

    struct DiagnosticInputBackendFixture {
        backend: DeviceRegistryInputDiagnosticBackend,
        calls: Arc<Mutex<Vec<&'static str>>>,
        segmented_swipe_actions: Arc<Mutex<Vec<SegmentedSwipeAction>>>,
    }

    fn diagnostic_input_backend(result: DeviceResult<()>) -> DiagnosticInputBackendFixture {
        diagnostic_input_backend_with(result, TouchBackendChoice::AdbShellInput, false)
    }

    fn diagnostic_input_backend_with(
        result: DeviceResult<()>,
        requested_backend: TouchBackendChoice,
        supports_segmented_swipe: bool,
    ) -> DiagnosticInputBackendFixture {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let segmented_swipe_actions = Arc::new(Mutex::new(Vec::new()));
        DiagnosticInputBackendFixture {
            backend: DeviceRegistryInputDiagnosticBackend::new(
                Box::new(RecordingInputBackend {
                    result,
                    calls: Arc::clone(&calls),
                    segmented_swipe_actions: Arc::clone(&segmented_swipe_actions),
                    supports_segmented_swipe,
                }),
                requested_backend,
            ),
            calls,
            segmented_swipe_actions,
        }
    }
    fn test_segmented_swipe_action() -> SegmentedSwipeAction {
        SegmentedSwipeAction {
            points: [(1095, 355), (105, 357), (105, 257)],
            horizontal_duration_ms: 200,
            corner_hold_ms: 150,
            brake_distance_px: 100,
            brake_duration_ms: 200,
            slope_in: 2,
            slope_out: 0,
        }
    }

    fn native_style_operation_error() -> DeviceError {
        DeviceError::transient(
            "command=\"adb -s 127.0.0.1:16384 shell input tap 10 20\" \
             exit_status=Some(1) stdout=\"fixture stdout line 1\nfixture stdout line 2\" \
             stderr=\"fixture stderr\"",
        )
    }

    // Workflow #257 DEVICE-DIAGNOSTIC-v1: specification criteria for the device wrapper.
    #[test]
    fn device_registry_input_operation_failure_matrix_preserves_error_and_context() {
        for operation in TEST_INPUT_OPERATIONS {
            let original = native_style_operation_error();
            let DiagnosticInputBackendFixture {
                mut backend, calls, ..
            } = diagnostic_input_backend(Err(original.clone()));
            let returned = operation
                .invoke(&mut backend)
                .expect_err("configured operation failure");
            assert_eq!(returned.severity(), original.severity());
            assert_eq!(returned.message(), original.message());
            assert_eq!(
                returned.diagnostic_message(),
                Some("device registry input operation failed")
            );
            assert_eq!(
                returned.is_fallback_eligible(),
                original.is_fallback_eligible()
            );
            let diagnostic = returned.diagnostic().expect("adapter diagnostic");
            assert_eq!(diagnostic.category(), DeviceErrorCategory::Native);
            assert_eq!(diagnostic.stage(), "device_registry.input.operation");
            let context = returned
                .diagnostic_context()
                .expect("adapter diagnostic context");
            assert_eq!(context.backend(), "adb_shell_input");
            assert_eq!(context.operation(), operation.name());
            assert_eq!(
                context.declared_sensitivity(),
                DeviceErrorSensitivity::Sensitive
            );
            assert_eq!(*calls.lock().expect("input calls"), [operation.name()]);
        }

        let original = DeviceError::transient("producer-owned private input failure")
            .with_diagnostic(DeviceErrorCategory::CommandWrite, "maatouch.stdin.write")
            .with_diagnostic_context("maatouch", "child_write", DeviceErrorSensitivity::Internal);
        let DiagnosticInputBackendFixture {
            mut backend, calls, ..
        } = diagnostic_input_backend(Err(original.clone()));
        let returned = TestInputOperation::Reset
            .invoke(&mut backend)
            .expect_err("producer-classified operation failure");
        assert_eq!(returned, original);
        assert_eq!(returned.message(), original.message());
        assert_eq!(returned.diagnostic_message(), None);
        let diagnostic = returned.diagnostic().expect("producer diagnostic");
        assert_eq!(diagnostic.category(), DeviceErrorCategory::CommandWrite);
        assert_eq!(diagnostic.stage(), "maatouch.stdin.write");
        let context = returned
            .diagnostic_context()
            .expect("producer diagnostic context");
        assert_eq!(context.backend(), "maatouch");
        assert_eq!(context.operation(), "child_write");
        assert_eq!(
            context.declared_sensitivity(),
            DeviceErrorSensitivity::Internal
        );
        assert_eq!(*calls.lock().expect("input calls"), ["reset"]);
    }

    // Test class: specification criterion.
    #[test]
    fn maatouch_write_failure_preserves_typed_diagnostic() {
        let original = DeviceError::transient("Broken pipe (os error 232)")
            .with_diagnostic(DeviceErrorCategory::CommandWrite, "maatouch.stdin.write");
        let DiagnosticInputBackendFixture {
            mut backend, calls, ..
        } = diagnostic_input_backend_with(
            Err(original.clone()),
            TouchBackendChoice::MaaTouch,
            true,
        );
        let returned = backend
            .swipe(10, 20, 30, 40, 50)
            .expect_err("controlled MaaTouch write failure");
        assert_eq!(returned, original);
        assert_eq!(returned.message(), original.message());
        assert_eq!(
            returned.diagnostic().expect("diagnostic").category(),
            DeviceErrorCategory::CommandWrite
        );
        assert_eq!(
            returned.diagnostic().expect("diagnostic").stage(),
            "maatouch.stdin.write"
        );
        let context = returned.diagnostic_context().expect("context");
        assert_eq!(context.backend(), "maatouch");
        assert_eq!(context.operation(), "swipe");
        assert_eq!(
            context.declared_sensitivity(),
            DeviceErrorSensitivity::Sensitive
        );
        assert_eq!(*calls.lock().expect("input calls"), ["swipe"]);
    }

    // Workflow #241 / DeviceRegistry segmented swipe v2. Specification criterion.
    #[test]
    fn device_registry_input_segmented_capability_matches_inner_backend() {
        for supported in [false, true] {
            let DiagnosticInputBackendFixture { backend, calls, .. } =
                diagnostic_input_backend_with(Ok(()), TouchBackendChoice::MaaTouch, supported);
            assert_eq!(backend.supports_segmented_swipe(), supported);
            assert!(calls.lock().expect("input calls").is_empty());
        }
    }

    // Workflow #241 / DeviceRegistry segmented swipe v2. Specification criterion.
    #[test]
    fn device_registry_input_segmented_swipe_forwards_exact_action() {
        let action = test_segmented_swipe_action();
        let DiagnosticInputBackendFixture {
            mut backend,
            calls,
            segmented_swipe_actions,
        } = diagnostic_input_backend_with(Ok(()), TouchBackendChoice::MaaTouch, true);
        backend
            .segmented_swipe(action)
            .expect("segmented swipe succeeds");
        assert_eq!(*calls.lock().expect("input calls"), ["segmented_swipe"]);
        assert_eq!(
            *segmented_swipe_actions
                .lock()
                .expect("segmented swipe actions"),
            [action]
        );
    }

    // Workflow #241 / DeviceRegistry segmented swipe v2. Specification criterion.
    #[test]
    fn device_registry_input_segmented_swipe_failure_preserves_error() {
        let action = test_segmented_swipe_action();
        let original = DeviceError::transient(
            "failed to write segmented swipe to fixture.device:16384: Broken pipe",
        )
        .with_diagnostic(DeviceErrorCategory::CommandWrite, "maatouch.stdin.write");
        let DiagnosticInputBackendFixture {
            mut backend,
            calls,
            segmented_swipe_actions,
        } = diagnostic_input_backend_with(
            Err(original.clone()),
            TouchBackendChoice::MaaTouch,
            true,
        );
        let returned = backend
            .segmented_swipe(action)
            .expect_err("segmented swipe failure");
        assert_eq!(returned, original);
        assert_eq!(returned.message(), original.message());
        assert_eq!(
            returned.diagnostic().expect("diagnostic").category(),
            DeviceErrorCategory::CommandWrite
        );
        assert_eq!(
            returned.diagnostic().expect("diagnostic").stage(),
            "maatouch.stdin.write"
        );
        assert_eq!(
            returned.diagnostic_context().expect("context").operation(),
            "segmented_swipe"
        );
        assert_eq!(*calls.lock().expect("input calls"), ["segmented_swipe"]);
        assert_eq!(
            *segmented_swipe_actions
                .lock()
                .expect("segmented swipe actions"),
            [action]
        );
    }

    #[test]
    fn device_registry_input_operation_success_matrix_delegates_once() {
        for operation in TEST_INPUT_OPERATIONS {
            let DiagnosticInputBackendFixture {
                mut backend, calls, ..
            } = diagnostic_input_backend(Ok(()));
            operation
                .invoke(&mut backend)
                .expect("configured operation success");
            assert_eq!(*calls.lock().expect("input calls"), [operation.name()]);
        }
    }

    #[test]
    fn fixture_input_operations_remain_unwrapped() {
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
                "alias": "neutral.fixture", "instance_id": id.transport(),
                "fixture_backend": {
                    "frames": [{"width": 1, "height": 1, "rgb": [1, 2, 3]}], "max_inputs": 1
                }
            }]
        });
        let config = serde_json::from_value::<ActingdConfigFile>(value).expect("typed config");
        let assembly = config.assemble().expect("runtime assembly");
        let mut backend =
            ExecutionBackendProvider::open_input(&assembly.registry, "neutral.fixture")
                .expect("fixture input");
        backend.tap(10, 20).expect("fixture input within budget");
        let error = backend
            .tap(10, 20)
            .expect_err("fixture input budget remains bounded");
        assert_eq!(error, DeviceError::fatal("fixture input budget exhausted"));
        assert!(error.diagnostic().is_none());
        assert!(error.diagnostic_context().is_none());
    }

    // C1B9 v16 D04: PR298 review 5120590779; CI33961302177 preserves the first red.
    // Endpoint privacy regression: Workflow #241 / #241-MAATOUCH-DIAGNOSTIC-PRIVACY-v3.
    // DEVICE-DIAGNOSTIC-v1: real missing-ADB failures use the configured provider and official IPC.
    #[test]
    fn formal_device_registry_failures_preserve_native_cause_and_public_privacy_in_ledger() {
        use actingcommand_contract::{
            EventActor, EventPayload, EventQuery, EventSource, EventType, InputAction,
            RuntimeErrorCode, RuntimePayload, Sensitivity,
        };
        use actingcommand_ledger::{GlobalLedger, GlobalLedgerEvidenceConfig};
        use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig};
        use actingcommand_runtime_host::RuntimeHost;

        for (capture, endpoint) in [
            (false, "127.0.0.1:16384"),
            (true, "198.51.100.42:16416"),
            (true, "[2001:db8::42]:16416"),
            (true, "device.example.test:16416"),
        ] {
            let root = TempDir::new().expect("tempdir");
            let id = IdentifierIssuer::new()
                .expect("issuer")
                .mint_instance_id()
                .expect("instance id");
            let missing_adb = root.path().join("missing-adb.exe");
            let value = json!({
                "schema_version": CONFIG_SCHEMA_VERSION,
                "state_root": root.path(),
                "bind_host": "127.0.0.1", "bind_port": 0,
                "secret_fingerprint_salt": "0123456789abcdef",
                "instances": [{
                    "alias": "neutral.device", "instance_id": id.transport(),
                    "application_id": "neutral.application", "adb_path": missing_adb,
                    "serial": endpoint, "connect": false,
                    "touch_backend": "adb_shell_input", "capture_backend": "adb"
                }]
            });
            let config = serde_json::from_value::<ActingdConfigFile>(value).expect("typed config");
            let assembly = config.assemble().expect("runtime assembly");
            let host = RuntimeHost::start(assembly.host, Arc::new(assembly.registry))
                .expect("runtime host");
            let client = RuntimeClient::connect(
                RuntimeClientConfig::new(root.path(), EventActor::Cli, EventSource::Cli)
                    .with_io_timeout(Duration::from_secs(2)),
            )
            .expect("official client");
            let error = if capture {
                client
                    .observe_readonly("neutral.device")
                    .expect_err("missing ADB capture fails")
            } else {
                let token = client.acquire_lease("neutral.device").expect("lease");
                client
                    .input(&token, InputAction::Reset)
                    .expect_err("missing ADB input open fails")
            };
            assert_eq!(
                error.projection().expect("typed error").code,
                if capture {
                    RuntimeErrorCode::CaptureFailed
                } else {
                    RuntimeErrorCode::BackendOpenFailed
                }
            );
            assert!(error.is_fatal());
            let receipt_text = format!("{error:?} {error}");
            assert!(!receipt_text.contains(endpoint));
            assert!(!receipt_text.contains("failed to spawn adb"));
            assert!(host.fatal_error().expect("health").is_none());
            drop(client);
            host.close().expect("close host after the request failure");
            let ledger =
                GlobalLedger::open_evidence(GlobalLedgerEvidenceConfig::new(root.path()), |_| None)
                    .expect("read closed authoritative ledger");
            assert!(ledger.corrupt_tail().is_none());
            let events = ledger.query(&EventQuery::default());
            let failure_type = if capture {
                EventType::CaptureFailed
            } else {
                EventType::InputFailed
            };
            let failures = events
                .iter()
                .filter(|event| event.event_type() == failure_type)
                .collect::<Vec<_>>();
            assert_eq!(failures.len(), 1);
            let failure = failures[0];
            let details = events
                .iter()
                .filter_map(|event| {
                    let EventPayload::Runtime(RuntimePayload::Failed(outcome)) = event.payload()
                    else {
                        return None;
                    };
                    outcome
                        .lifecycle_failure()
                        .filter(|detail| detail.native_detail().is_some())
                        .map(|detail| (event, detail))
                })
                .collect::<Vec<_>>();
            assert_eq!(details.len(), 1, "one private native cause per operation");
            let (event, lifecycle) = details[0];
            assert_eq!(lifecycle.entered_event_id(), Some(*failure.event_id()));
            assert_eq!(
                event.links().correlation_id(),
                failure.links().correlation_id()
            );
            assert_eq!(event.sensitivity(), Sensitivity::Sensitive);
            let detail = lifecycle.primary_detail().expect("actual device context");
            assert_eq!(detail.category(), "native");
            assert_eq!(detail.stage(), "adb.ensure_device.get_state");
            assert_eq!(
                detail.backend(),
                if capture {
                    "adb_screencap"
                } else {
                    "adb_shell_input"
                }
            );
            assert_eq!(detail.operation(), "ensure_device");
            assert_eq!(
                detail.declared_sensitivity(),
                if capture {
                    Sensitivity::Sensitive
                } else {
                    Sensitivity::Internal
                }
            );
            let native = lifecycle.native_detail().expect("native cause");
            assert!(!native.truncated());
            assert!(native.text().contains("failed to spawn adb"));
            assert!(native.text().contains(endpoint));
            if !capture {
                assert!(native.text().contains("child_operation=ensure_device"));
            }
            let public = serde_json::to_string(&event.payload().public_projection())
                .expect("public payload");
            assert!(!public.contains(endpoint));
            assert!(!public.contains("failed to spawn adb"));
            assert!(!public.contains("missing-adb.exe"));
        }
    }

    #[test]
    fn device_registry_input_open_success_preserves_result() {
        let value = open_device_registry_input_with_diagnostic(
            Some(TouchBackendChoice::AdbShellInput),
            || Ok::<_, DeviceError>(7_u8),
        )
        .expect("device-registry open success");
        assert_eq!(value, 7);
    }

    // Workflow #239 / #239-IMP-v2 (comment 5442382418): authorized Defect regression.
    #[test]
    fn device_registry_capture_open_failure_preserves_complete_diagnostic() {
        let original = DeviceError::transient("synthetic capture open failure");
        let returned = open_device_registry_capture_with_diagnostic(
            Some(CaptureBackendChoice::NemuIpc),
            || Err::<u8, _>(original.clone()),
        )
        .expect_err("capture open failure");
        assert_eq!(returned.severity(), original.severity());
        assert_eq!(returned.message(), original.message());
        assert_eq!(
            returned.diagnostic_message(),
            Some("device registry capture open failed")
        );
        let diagnostic = returned.diagnostic().expect("capture adapter diagnostic");
        assert_eq!(diagnostic.category(), DeviceErrorCategory::Native);
        assert_eq!(diagnostic.stage(), "device_registry.capture.open");
        let context = returned
            .diagnostic_context()
            .expect("capture adapter diagnostic context");
        assert_eq!(context.backend(), "nemu_ipc");
        assert_eq!(context.operation(), "open_capture");
        assert_eq!(
            context.declared_sensitivity(),
            DeviceErrorSensitivity::Sensitive
        );

        let producer = DeviceError::fatal("producer capture failure")
            .with_diagnostic(DeviceErrorCategory::Protocol, "nemu.target.resolve")
            .with_diagnostic_context(
                "nemu_ipc",
                "target_resolve",
                DeviceErrorSensitivity::Internal,
            );
        let returned =
            open_device_registry_capture_with_diagnostic(Some(CaptureBackendChoice::Adb), || {
                Err::<u8, _>(producer.clone())
            })
            .expect_err("producer-classified capture failure");
        assert_eq!(returned, producer);
        assert_eq!(returned.message(), producer.message());
        assert_eq!(returned.diagnostic_message(), None);
        let diagnostic = returned.diagnostic().expect("producer diagnostic");
        assert_eq!(diagnostic.category(), DeviceErrorCategory::Protocol);
        assert_eq!(diagnostic.stage(), "nemu.target.resolve");
        let context = returned
            .diagnostic_context()
            .expect("producer diagnostic context");
        assert_eq!(context.backend(), "nemu_ipc");
        assert_eq!(context.operation(), "target_resolve");
        assert_eq!(
            context.declared_sensitivity(),
            DeviceErrorSensitivity::Internal
        );
    }

    // Workflow #239 / #239-IMP-v2 (comment 5442382418): specification criterion.
    #[test]
    fn device_registry_capture_open_success_preserves_result() {
        let value = open_device_registry_capture_with_diagnostic(
            Some(CaptureBackendChoice::NemuIpc),
            || Ok::<_, DeviceError>(7_u8),
        )
        .expect("device-registry capture open success");
        assert_eq!(value, 7);
    }
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
        assert_eq!(
            assembly.registry.device_input_backends.get("node.a"),
            Some(&TouchBackendChoice::MaaTouch)
        );
        assert_eq!(
            assembly.registry.device_capture_backends.get("node.a"),
            Some(&CaptureBackendChoice::Adb)
        );
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
        assert_eq!(
            assembly.registry.mode_for_alias("neutral.fixture"),
            Some(ScheduledExecutionMode::FixtureSimulation)
        );
        assert!(assembly.registry.vision_provider().is_none());

        let config = serde_json::from_value::<ActingdConfigFile>(fixture(MAX_FIXTURE_INPUTS + 1))
            .expect("typed fixture config");
        assert_eq!(config.assemble().err(), Some("fixture_backend_invalid"));
    }

    #[test]
    fn configured_vision_provider_failure_is_recorded_before_runtime_ready() {
        use actingcommand_contract::{EventPayload, EventType, ProviderStartupObservation};
        use actingcommand_ledger::{GlobalLedger, GlobalLedgerEvidenceConfig};
        use actingcommand_ledger_forensics::{
            ForensicEventFilter, ForensicEventsRequest, ForensicOutput, ForensicReport,
            ForensicRequest,
        };
        use actingcommand_runtime_host::RuntimeHost;

        let root = TempDir::new().expect("tempdir");
        let id = IdentifierIssuer::new()
            .expect("issuer")
            .mint_instance_id()
            .expect("instance id");
        fs::write(root.path().join("invalid.json"), b"{}").expect("invalid manifest fixture");
        fs::write(
            root.path().join("missing-backend.json"),
            serde_json::to_vec(&json!({
                "schema_version": VISION_PROVIDER_ARTIFACTS_SCHEMA_VERSION,
                "onnxruntime": {
                    "provider_library_path": "missing-provider.dll",
                    "model_path": "model.onnx",
                    "model_ref": "neutral.model",
                "execution_provider": "cpu",
                    "model_sha256": "a".repeat(64),
                    "labels": ["neutral"],
                    "default_timeout_ms": 1000
                }
            }))
            .expect("manifest JSON"),
        )
        .expect("missing backend manifest");
        for (index, manifest, expected) in [
            (
                0,
                Some("missing.json"),
                Some("vision_provider_manifest_unavailable"),
            ),
            (
                1,
                Some("invalid.json"),
                Some("vision_provider_manifest_invalid"),
            ),
            (
                2,
                Some("missing-backend.json"),
                Some("vision_provider_unavailable"),
            ),
            (3, None, None),
        ] {
            let state_root = root.path().join(format!("state-{index}"));
            let mut config = serde_json::from_value::<ActingdConfigFile>(json!({
                "schema_version": CONFIG_SCHEMA_VERSION,
                "state_root": state_root,
                "bind_host": "127.0.0.1",
                "secret_fingerprint_salt": "0123456789abcdef",
                "vision_provider_manifest": manifest,
                "instances": [{
                    "alias": "neutral.fixture",
                    "instance_id": id.transport(),
                    "fixture_backend": {
                        "frames": [{"width": 1, "height": 1, "rgb": [1, 2, 3]}],
                        "max_inputs": 0
                    }
                }]
            }))
            .expect("typed config");
            config.source_root = root.path().to_path_buf();
            let assembly = config
                .assemble()
                .expect("configuration does not assemble a provider");
            assert!(!state_root.join("ledger").exists());
            let started = RuntimeHost::start_with_provider(assembly.host, |startup| {
                assembly.registry.assemble_provider(startup)
            });
            match (started, expected) {
                (Err(error), Some(expected)) => assert_eq!(error.code(), expected),
                (Ok(host), None) => host.close().expect("close host"),
                (Ok(host), Some(_)) => {
                    host.close().expect("close unexpected host");
                    panic!("provider failure must prevent ready");
                }
                (Err(error), None) => panic!("fixture startup failed: {error}"),
            }
            assert!(
                !state_root
                    .join(actingcommand_contract::RUNTIME_INFO_FILE)
                    .exists()
            );
            let snapshot =
                GlobalLedger::open_evidence(GlobalLedgerEvidenceConfig::new(&state_root), |_| None)
                    .expect("startup ledger remains readable");
            let observations = snapshot
                .events()
                .iter()
                .filter_map(|event| {
                    if let EventPayload::Provider(payload) = event.payload() {
                        Some((event, &payload.record))
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>();
            assert!(!observations.is_empty());
            let provider_terminal = observations.last().expect("provider terminal");
            if expected.is_some() {
                let ProviderStartupObservation::Failed { failure, .. } =
                    &provider_terminal.1.observation
                else {
                    panic!("native failure missing")
                };
                assert!(!failure.module.is_empty());
                assert!(!failure.code.is_empty());
                assert!(!failure.message.is_empty());
                let public =
                    serde_json::to_string(&provider_terminal.0.payload().public_projection())
                        .expect("public projection");
                assert!(!public.contains(&failure.message));
                assert!(!snapshot.events().iter().any(|event| matches!(
                    event.event_type(),
                    EventType::RuntimeStarted | EventType::RuntimeTakeover
                )));
            } else {
                assert_eq!(
                    provider_terminal.1.observation,
                    ProviderStartupObservation::NotConfigured
                );
                let started = snapshot
                    .events()
                    .iter()
                    .find(|event| event.event_type() == EventType::RuntimeStarted)
                    .expect("daemon start");
                assert!(provider_terminal.0.sequence() < started.sequence());
            }
            let through = snapshot.latest_sequence();
            let request = ForensicEventsRequest::new(
                ForensicEventFilter::new(Some("provider".into()), None, None, None)
                    .expect("provider filter"),
                0,
                Some(through),
                2,
            )
            .expect("bounded page");
            let ForensicOutput::Machine(ForensicReport::Events(page)) =
                actingcommand_ledger_forensics::run(ForensicRequest::events(&state_root, request))
                    .expect("B read-only startup page")
            else {
                panic!("event page")
            };
            assert_eq!(page.through_sequence, through);
            assert_eq!(
                page.events,
                observations
                    .iter()
                    .take(2)
                    .map(|(event, _)| (*event).clone())
                    .collect::<Vec<_>>()
            );
            assert_eq!(page.next_after_sequence.is_some(), observations.len() > 2);
        }
    }
    fn absolute_artifact_root(label: &str) -> PathBuf {
        #[cfg(windows)]
        {
            PathBuf::from(format!(r"C:\synthetic-artifact-root\{label}"))
        }
        #[cfg(not(windows))]
        {
            PathBuf::from(format!("/synthetic-artifact-root/{label}"))
        }
    }

    fn fastdeploy_manifest(
        runtime_library_paths: Vec<PathBuf>,
        runtime_library_path: PathBuf,
    ) -> VisionProviderArtifactManifest {
        VisionProviderArtifactManifest {
            schema_version: VISION_PROVIDER_ARTIFACTS_SCHEMA_VERSION.to_string(),
            fastdeploy_ppocr: Some(FastDeployPpocrArtifacts {
                provider_library_path: PathBuf::from("provider.dll"),
                provider_library_sha256: None,
                runtime_library_paths,
                runtime_library_path: Some(runtime_library_path),
                runtime_library_sha256: None,
                detector_model_path: PathBuf::from("detector.onnx"),
                recognizer_model_path: PathBuf::from("recognizer.onnx"),
                dictionary_path: PathBuf::from("dictionary.txt"),
                classifier_model_path: None,
                model_ref: None,
                model_sha256: None,
                detector_model_sha256: None,
                recognizer_model_sha256: None,
                dictionary_sha256: None,
                classifier_model_sha256: None,
                execution_provider: Some(OnnxExecutionProvider::Cpu),
                cuda_device: None,
                strict_no_fallback: Some(true),
                supported_languages: vec!["neutral".to_string()],
                default_timeout_ms: 1_000,
            }),
            onnxruntime: None,
        }
    }

    #[test]
    fn fastdeploy_selected_runtime_identity_resolves_with_relative_closure() {
        let artifact_root = absolute_artifact_root("relative");
        let mut manifest = fastdeploy_manifest(
            vec![
                PathBuf::from("runtime/companion.dll"),
                PathBuf::from("runtime/onnxruntime.dll"),
            ],
            PathBuf::from("runtime/onnxruntime.dll"),
        );

        resolve_vision_artifact_paths(&mut manifest, &artifact_root);

        let artifacts = manifest.fastdeploy_ppocr.as_ref().expect("OCR artifacts");
        assert_eq!(
            artifacts.runtime_library_paths,
            [
                artifact_root.join("runtime/companion.dll"),
                artifact_root.join("runtime/onnxruntime.dll"),
            ]
        );
        assert_eq!(
            artifacts
                .onnxruntime_library_path()
                .expect("selected runtime identity"),
            artifact_root.join("runtime/onnxruntime.dll")
        );
    }

    #[test]
    fn fastdeploy_absolute_runtime_identity_and_closure_remain_unchanged() {
        let artifact_root = absolute_artifact_root("unused");
        let runtime_root = absolute_artifact_root("runtime");
        let runtime_paths = vec![
            runtime_root.join("companion.dll"),
            runtime_root.join("onnxruntime.dll"),
        ];
        let selected = runtime_paths[1].clone();
        let mut manifest = fastdeploy_manifest(runtime_paths.clone(), selected.clone());

        resolve_vision_artifact_paths(&mut manifest, &artifact_root);

        let artifacts = manifest.fastdeploy_ppocr.as_ref().expect("OCR artifacts");
        assert_eq!(artifacts.runtime_library_paths, runtime_paths);
        assert_eq!(artifacts.runtime_library_path.as_ref(), Some(&selected));
        assert_eq!(
            artifacts
                .onnxruntime_library_path()
                .expect("selected runtime identity"),
            selected
        );
    }

    #[test]
    fn fastdeploy_runtime_identity_mismatch_remains_fail_closed() {
        let artifact_root = absolute_artifact_root("mismatch");
        let mut manifest = fastdeploy_manifest(
            vec![PathBuf::from("runtime/companion.dll")],
            PathBuf::from("runtime/onnxruntime.dll"),
        );

        resolve_vision_artifact_paths(&mut manifest, &artifact_root);

        let error = manifest
            .fastdeploy_ppocr
            .as_ref()
            .expect("OCR artifacts")
            .onnxruntime_library_path()
            .expect_err("selected runtime outside closure rejected");
        assert!(
            error
                .message()
                .contains("runtime_library_path must occur exactly once")
        );
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
    fn mixed_registry_keeps_device_and_fixture_modes_explicit_per_instance() {
        let issuer = IdentifierIssuer::new().expect("issuer");
        let device_id = issuer.mint_instance_id().expect("device instance id");
        let fixture_id = issuer.mint_instance_id().expect("fixture instance id");
        let value = json!({
            "schema_version": CONFIG_SCHEMA_VERSION,
            "state_root": "state",
            "bind_host": "127.0.0.1",
            "secret_fingerprint_salt": "0123456789abcdef",
            "instances": [
                {
                    "alias": "neutral.device",
                    "instance_id": device_id.transport(),
                    "application_id": "neutral.application",
                    "adb_path": "must-not-open",
                    "touch_backend": "adb",
                    "capture_backend": "adb"
                },
                {
                    "alias": "neutral.fixture",
                    "instance_id": fixture_id.transport(),
                    "fixture_backend": {
                        "frames": [{"width": 1, "height": 1, "rgb": [1, 2, 3]}],
                        "max_inputs": 0
                    }
                }
            ]
        });
        let config =
            serde_json::from_value::<ActingdConfigFile>(value.clone()).expect("typed config");
        let assembly = config.assemble().expect("mixed registry assembly");
        assert_eq!(
            assembly.registry.mode_for_alias("neutral.device"),
            Some(ScheduledExecutionMode::DeviceRegistry)
        );
        assert_eq!(
            assembly.registry.mode_for_alias("neutral.fixture"),
            Some(ScheduledExecutionMode::FixtureSimulation)
        );
        assert_eq!(assembly.registry.instance_aliases().len(), 2);
        for alias in [
            "Neutral.Device".to_owned(),
            " Device Ω ".to_owned(),
            "é".repeat(128),
            " ".to_owned(),
        ] {
            let mut changed = value.clone();
            changed["instances"][0]["alias"] = json!(alias);
            changed["instances"][1]["alias"] = json!(" Fixture Ω ");
            let assembly = serde_json::from_value::<ActingdConfigFile>(changed)
                .expect("alias config")
                .assemble()
                .expect("registered aliases");
            assert_eq!(
                assembly.registry.mode_for_alias(&alias),
                Some(ScheduledExecutionMode::DeviceRegistry)
            );
            assert_eq!(
                assembly.registry.resolve(&alias).unwrap().instance_id(),
                *device_id.transport()
            );
            assert_eq!(
                assembly.registry.mode_for_alias(" Fixture Ω "),
                Some(ScheduledExecutionMode::FixtureSimulation)
            );
            assert!(assembly.registry.resolve(" fixture Ω ").is_none());
            assert!(
                assembly
                    .registry
                    .mode_for_alias("unknown.instance")
                    .is_none()
            );
        }
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
