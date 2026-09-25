// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_contract::resource_declaration::{
    ProcedureBindingConfigFile, ScheduledExecutionConfigFile,
};
use actingcommand_contract::{
    ContainedTaskRequest, InstanceId, InstanceResourcePackage, InstanceResourcePackageKind,
    RuntimeConfigManifest,
};
use actingcommand_device::{
    AdbConfig, CaptureBackendChoice, CaptureBackendConfig, CaptureBackendName, DeviceTarget, Frame,
    MaaTouchConfig, MinitouchConfig, PixelFormat, TouchBackendChoice, TouchBackendConfig,
};
use actingcommand_execution_kernel::{ExternalExpectedSha256, PreparedContainedTask};
use actingcommand_policy::{
    CatalogDocumentSource, CatalogSources, EvaluationFacts, EvaluationResources, MAX_APPROVAL_REFS,
    MAX_CATALOG_BYTES, MAX_DOCUMENT_BYTES, MAX_REFERENCES_PER_TASK, MAX_TASKS, compile_catalog,
};
use actingcommand_runtime_host::{
    AgentDispatcherConfig, DiscoverySpec, ExecutionBackendProvider, ExecutionBackendRegistration,
    ExecutionBackendRegistry, FixtureInstanceSpec, InstanceMode, InstanceSpec,
    PerformanceMonitorConfig, PolicyCadence, PolicyInputSnapshot, ProcedureBinding,
    ProcedureManifest, ProviderAssembly, RecognitionVisionProvider, RuntimeHostConfig,
    RuntimeHostError, VisionFfiProvider, VisionModelIdentity, VisionSpec,
};
use actingcommand_vision_ffi::{
    NnEngine, OcrEngine, VISION_PROVIDER_ARTIFACTS_SCHEMA_VERSION, VisionProviderArtifactManifest,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
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
    /// Defaulted scalars stay `Option` so the manifest can tell an explicit value
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
    frame_retention_failed_run_successes: Option<u16>,
    #[serde(default)]
    frame_retention_failed_run_days: Option<u16>,
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
    /// Workflow #318 (cfg2): performance monitor tunables that had no file field.
    #[serde(default)]
    performance: Option<PerformanceConfigFile>,
    /// Workflow #318 (cfg2): daemon-level device tool paths, applied to every device
    /// instance's backend configuration; absent fields keep today's env / discovery /
    /// bundled-tool behaviour.
    #[serde(default)]
    device_paths: Option<DevicePathsConfigFile>,
    instances: Vec<InstanceConfig>,
    #[serde(skip)]
    source_root: PathBuf,
}

/// `PerformanceMonitorConfig` pressure streaks (`1..=30`, default 3 each).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PerformanceConfigFile {
    #[serde(default)]
    pressure_start_samples: Option<u16>,
    #[serde(default)]
    pressure_end_samples: Option<u16>,
}

/// The upper bound `PerformanceMonitorConfig::validate` applies to both pressure streaks;
/// checked here so the refusal carries `invalid_pressure_samples` before host validation.
const MAX_PRESSURE_STREAK_SAMPLES: u16 = 30;

/// Daemon-level device tool paths. Each set path must be absolute and exist
/// (`device_path_invalid`); it is then passed to the backend configuration it names.
#[derive(Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct DevicePathsConfigFile {
    /// `NemuIpcConfig.nemu_folder`: the MuMu installation root Nemu IPC connects to.
    #[serde(default)]
    nemu_folder: Option<PathBuf>,
    /// `NemuIpcConfig.dll_path`: the Nemu IPC capture DLL.
    #[serde(default)]
    nemu_ipc_dll: Option<PathBuf>,
    /// `DroidcastRawConfig.local_apk`: the DroidCast_raw APK pushed to the device.
    #[serde(default)]
    droidcast_apk: Option<PathBuf>,
    /// `MinitouchConfig.local_path`; a per-instance `minitouch_local_path` wins over it.
    #[serde(default)]
    minitouch_path: Option<PathBuf>,
    /// `MaaTouchConfig.local_path`; a per-instance `maatouch_local_path` wins over it.
    #[serde(default)]
    maatouch_path: Option<PathBuf>,
}

impl DevicePathsConfigFile {
    /// Every configured path with its manifest name, in a fixed order.
    fn entries(&self) -> [(&'static str, Option<&Path>); 5] {
        [
            ("nemu_folder", self.nemu_folder.as_deref()),
            ("nemu_ipc_dll", self.nemu_ipc_dll.as_deref()),
            ("droidcast_apk", self.droidcast_apk.as_deref()),
            ("minitouch_path", self.minitouch_path.as_deref()),
            ("maatouch_path", self.maatouch_path.as_deref()),
        ]
    }

    /// A configured path must be absolute and exist; nothing is opened or resolved.
    fn validate(&self) -> Result<(), &'static str> {
        for (_, path) in self.entries() {
            if let Some(path) = path
                && (path.as_os_str().is_empty()
                    || !path.is_absolute()
                    || fs::metadata(path).is_err())
            {
                return Err("device_path_invalid");
            }
        }
        Ok(())
    }
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

#[derive(Clone, Deserialize)]
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
    /// Slice #324-r1: the instance's default resource package, a local package file or
    /// package directory. A relative path resolves against the configuration file's
    /// directory, as `startup_package.package` does; see `validate_resource_packages`.
    #[serde(default)]
    resource_package: Option<PathBuf>,
    /// Slice #316-B4: `false` turns the stuck-recovery ladder off for this instance
    /// (default `true`).
    #[serde(default)]
    stuck_recovery: Option<bool>,
    /// Slice #316-B4: at most one stuck-recovery ladder per this many seconds (default 600,
    /// `1..=86400`).
    #[serde(default)]
    stuck_recovery_cooldown_secs: Option<u32>,
    #[serde(default)]
    fixture_backend: Option<FixtureBackendConfigFile>,
    /// The daemon-level `device_paths`, copied in by `assemble` so a deferred instance
    /// registered after discovery applies the same paths as an explicit one.
    #[serde(skip)]
    device_paths: DevicePathsConfigFile,
}

/// Same semantics as `actingctl task-run --package <locator> --expected-sha256 <hex>`: the
/// locator (relative paths resolve against the configuration file's directory) and the
/// bare lowercase hex digest. The file is neither opened nor hashed at assembly.
#[derive(Clone, Deserialize)]
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

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureBackendConfigFile {
    frames: Vec<FixtureFrameConfigFile>,
    max_inputs: u16,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureFrameConfigFile {
    width: u32,
    height: u32,
    rgb: Vec<u8>,
}

pub(super) struct RuntimeAssembly {
    pub(super) host: RuntimeHostConfig,
    pub(super) provider: ConfiguredProvider,
    pub(super) policy: Option<PolicyBootstrap>,
    /// The manifest handed to `host`; `check-config` prints it.
    pub(super) manifest: RuntimeConfigManifest,
    /// Per instance alias: the configured `resource_package`, resolved but not yet admitted.
    pub(super) resource_packages: BTreeMap<String, PathBuf>,
}

/// A refused `resource_package`: the code, the offending instance and path and, for
/// `resource_package_invalid`, the package loader's own code and message.
pub(super) struct ResourcePackageRejection {
    pub(super) code: &'static str,
    alias: String,
    path: PathBuf,
    loader: Option<(&'static str, String)>,
}

impl ResourcePackageRejection {
    pub(super) fn detail(&self) -> serde_json::Value {
        serde_json::json!({
            "alias": self.alias,
            "path": self.path.to_string_lossy(),
            "loader_code": self.loader.as_ref().map(|(code, _)| code),
            "loader_message": self.loader.as_ref().map(|(_, message)| message),
        })
    }
}

impl std::fmt::Display for ResourcePackageRejection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "instance {:?} resource_package {:?}",
            self.alias,
            self.path.to_string_lossy()
        )?;
        if let Some((code, message)) = &self.loader {
            write!(formatter, "; loader {code}: {message}")?;
        }
        Ok(())
    }
}

/// Admits every configured `resource_package`, identically for `check-config` and daemon
/// startup: the path must exist (`resource_package_missing`); a file must be read by the
/// contained-task package loader exactly as `task-run` admits one, with the file's own
/// digest as the expected one (`resource_package_invalid`). A directory is confirmed to
/// exist only: the loader reads a package directory solely against a Git source-tree
/// reference, which this field does not carry, so `check-config` lists it as not checked.
pub(super) fn validate_resource_packages(
    configured: &BTreeMap<String, PathBuf>,
) -> Result<BTreeMap<String, InstanceResourcePackage>, ResourcePackageRejection> {
    let mut admitted = BTreeMap::new();
    for (alias, configured_path) in configured {
        let rejected = |code, path: &Path, loader| ResourcePackageRejection {
            code,
            alias: alias.clone(),
            path: path.to_path_buf(),
            loader,
        };
        let path = std::path::absolute(configured_path)
            .map_err(|_| rejected("resource_package_missing", configured_path, None))?;
        let metadata =
            fs::metadata(&path).map_err(|_| rejected("resource_package_missing", &path, None))?;
        let kind = if metadata.is_dir() {
            InstanceResourcePackageKind::Directory
        } else if metadata.is_file() {
            let invalid = |code, message: String| {
                rejected("resource_package_invalid", &path, Some((code, message)))
            };
            let bytes = fs::read(&path)
                .map_err(|error| invalid("package_read_failed", error.to_string()))?;
            let expected =
                ExternalExpectedSha256::parse_hex(&format!("{:x}", Sha256::digest(&bytes)))
                    .map_err(|error| invalid("package_reference_invalid", error.to_string()))?;
            PreparedContainedTask::load(alias, &bytes, expected).map_err(|error| {
                invalid(
                    error.code(),
                    error
                        .detail()
                        .map_or_else(|| error.to_string(), str::to_owned),
                )
            })?;
            InstanceResourcePackageKind::File
        } else {
            return Err(rejected("resource_package_invalid", &path, None));
        };
        admitted.insert(
            alias.clone(),
            InstanceResourcePackage {
                path: path.to_string_lossy().into_owned(),
                kind,
            },
        );
    }
    Ok(admitted)
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

/// The parsed provider configuration: the typed instance specs the registry consumes plus
/// what provider startup still resolves (discovery-bound instances, the vision manifest).
/// `assemble` registers nothing and spawns nothing; `ExecutionBackendRegistry::from_assembly`
/// is the one place instances are registered.
pub(super) struct ConfiguredProvider {
    /// Explicit device and fixture instances.
    instances: Vec<InstanceSpec>,
    /// Instances bound by `instance_index`/`instance_name` after one `MuMuManager` discovery
    /// run at provider startup.
    deferred: Vec<DeferredInstance>,
    /// `(source_root, configured manifest path)` of the vision provider, read at startup.
    vision_manifest: Option<(PathBuf, PathBuf)>,
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
    /// The same declaration registered with a stand-in ADB target: `check-config` validates
    /// it through the registry exactly as an explicit entry. Never dispatched.
    stand_in: InstanceSpec,
}

enum ConfiguredInstance {
    Spec(InstanceSpec),
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
        let defaults = actingcommand_contract::FailedRunRetentionPolicy::default();
        let failed_run_retention = actingcommand_contract::FailedRunRetentionPolicy {
            successor_successes: self
                .frame_retention_failed_run_successes
                .unwrap_or(defaults.successor_successes),
            retention_days: self
                .frame_retention_failed_run_days
                .unwrap_or(defaults.retention_days),
        };
        failed_run_retention
            .validate()
            .map_err(|_| "invalid_failed_run_retention_policy")?;
        let mut performance_monitor = PerformanceMonitorConfig::default();
        let (pressure_start_samples, pressure_end_samples) =
            self.performance
                .as_ref()
                .map_or((None, None), |performance| {
                    (
                        performance.pressure_start_samples,
                        performance.pressure_end_samples,
                    )
                });
        for samples in [pressure_start_samples, pressure_end_samples]
            .into_iter()
            .flatten()
        {
            if !(1..=MAX_PRESSURE_STREAK_SAMPLES).contains(&samples) {
                return Err("invalid_pressure_samples");
            }
        }
        if let Some(samples) = pressure_start_samples {
            performance_monitor = performance_monitor.with_pressure_start_samples(samples);
        }
        if let Some(samples) = pressure_end_samples {
            performance_monitor = performance_monitor.with_pressure_end_samples(samples);
        }
        let device_paths = self.device_paths.unwrap_or_default();
        device_paths.validate()?;
        let mut instances = self.instances;
        let mut startup_packages = BTreeMap::new();
        let mut resource_packages = BTreeMap::new();
        let mut stuck_recovery = BTreeMap::new();
        for instance in &mut instances {
            instance.device_paths = device_paths.clone();
            let settings = actingcommand_contract::InstanceStuckRecovery {
                enabled: instance.stuck_recovery.unwrap_or(true),
                cooldown_secs: instance.stuck_recovery_cooldown_secs.unwrap_or(
                    actingcommand_contract::InstanceStuckRecovery::DEFAULT_COOLDOWN_SECS,
                ),
            };
            settings
                .validate()
                .map_err(|_| "stuck_recovery_cooldown_invalid")?;
            stuck_recovery.insert(instance.alias.clone(), settings);
            if let Some(path) = instance.resource_package.take() {
                // An empty path stays empty so admission reports it missing.
                let path = if path.as_os_str().is_empty() || path.is_absolute() {
                    path
                } else {
                    self.source_root.join(path)
                };
                resource_packages.insert(instance.alias.clone(), path);
            }
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
        let instances = instances
            .into_iter()
            .map(InstanceConfig::backend)
            .collect::<Result<Vec<_>, _>>()?;
        let provider = ConfiguredProvider::new(
            instances,
            self.mumu_root,
            self.vision_provider_manifest
                .map(|path| (self.source_root.clone(), path)),
        );
        let policy = self
            .policy
            .map(|policy| policy.assemble(&self.source_root))
            .transpose()?;
        if let Some(policy) = policy.as_ref() {
            policy.validate_registry_modes(&provider)?;
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
                .with_frame_retention_enabled(self.frame_retention_enabled.unwrap_or(true))
                .with_failed_run_retention(failed_run_retention)
                .with_bind_address(SocketAddr::new(
                    bind_host,
                    self.bind_port.unwrap_or_default(),
                ))
                .with_policy_cadence(policy_cadence.clone())
                .with_performance_monitor(performance_monitor);
        let instances_startup_package_count = startup_packages.len();
        host = host
            .with_startup_packages(startup_packages)
            .with_stuck_recovery(stuck_recovery);
        // Every effective value is read back from `host`; the file only says what it named.
        let manifest = manifest::build(&manifest::ManifestInputs {
            host: &host,
            bind_port_explicit: self.bind_port.is_some(),
            device_diagnostic_mode_explicit: self.device_diagnostic_mode.is_some(),
            frame_retention_enabled: self.frame_retention_enabled,
            failed_run_successes_explicit: self.frame_retention_failed_run_successes.is_some(),
            failed_run_days_explicit: self.frame_retention_failed_run_days.is_some(),
            capacity_thresholds_explicit: self.capacity_thresholds.is_some(),
            pressure_start_samples_explicit: pressure_start_samples.is_some(),
            pressure_end_samples_explicit: pressure_end_samples.is_some(),
            secret_fingerprint_salt_bytes: self.secret_fingerprint_salt.len(),
            mumu_root: provider.mumu_root(),
            device_paths: device_paths.entries(),
            governance_configured: self.governance_capability.is_some(),
            agent_dispatcher: agent_dispatcher_budget,
            policy_configured: policy.is_some(),
            vision_provider_configured: provider.vision_manifest.is_some(),
            instances_count: provider.instance_count(),
            instances_deferred_count: provider.deferred.len(),
            instances_startup_package_count,
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
                registry_modes: provider.modes(),
                cadence: policy_cadence,
            })
        } else {
            None
        };
        Ok(RuntimeAssembly {
            host,
            provider,
            policy,
            manifest,
            resource_packages,
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
    fn validate_registry_modes(&self, provider: &ConfiguredProvider) -> Result<(), &'static str> {
        for (procedure_ref, instance_alias) in &self.scheduled_instance_scopes {
            let scheduled = self
                .scheduled_tasks
                .get(procedure_ref)
                .ok_or("scheduled_execution_binding_missing")?;
            let actual = provider
                .mode_for_alias(instance_alias)
                .ok_or("scheduled_execution_instance_unknown")?;
            if actual != scheduled.mode {
                return Err("scheduled_execution_backend_mode_mismatch");
            }
        }
        let modes = provider.modes();
        for scheduled in self.scheduled_tasks.values() {
            if !modes.values().any(|mode| mode == &scheduled.mode) {
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
    fn backend(self) -> Result<ConfiguredInstance, &'static str> {
        if self.fixture_backend.is_some() {
            self.fixture_backend().map(ConfiguredInstance::Spec)
        } else if let Some(key) = self.binding_key()? {
            self.deferred_backend(key)
        } else {
            self.device_backend().map(ConfiguredInstance::Spec)
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
    fn deferred_backend(self, key: InstanceBindingKey) -> Result<ConfiguredInstance, &'static str> {
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
        // The remaining registration rules need no discovery result either (the
        // `nemu_app_index` pairing, alias and application identity): run the same
        // `device_registration` provider startup binds with. Only the ADB target is a
        // stand-in, replaced by the discovered one at startup; `check-config` registers the
        // stand-in so the registry judges the declaration exactly as an explicit entry.
        let stand_in = InstanceSpec::real(self.clone().device_registration(
            "adb".to_owned(),
            default_device_host(),
            None,
        )?);
        Ok(ConfiguredInstance::Deferred(Box::new(DeferredInstance {
            alias: self.alias.clone(),
            key,
            config: self,
            stand_in,
        })))
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

    fn device_backend(self) -> Result<InstanceSpec, &'static str> {
        let adb_path = self.adb_path.clone().ok_or("instance_config_invalid")?;
        let host = self.host.clone().unwrap_or_else(default_device_host);
        let port = self.port.unwrap_or_else(default_device_port);
        self.device_registration(adb_path, host, Some(port))
            .map(InstanceSpec::real)
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
    ) -> Result<ExecutionBackendRegistration, &'static str> {
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
        // Daemon-level `device_paths` first; the per-instance path keeps precedence.
        if let Some(path) = &self.device_paths.maatouch_path {
            maatouch.local_path = path.clone();
        }
        if let Some(path) = &self.device_paths.minitouch_path {
            minitouch.local_path = path.clone();
        }
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
        let mut capture = CaptureBackendConfig::new(adb.clone(), target.clone())
            .with_requested(capture_requested);
        // Absent daemon-level paths leave the backend defaults (env var, discovery) intact.
        if let Some(path) = &self.device_paths.droidcast_apk {
            capture.droidcast.local_apk = Some(path.clone());
        }
        if let Some(path) = &self.device_paths.nemu_folder {
            capture.nemu.nemu_folder = Some(path.clone());
        }
        if let Some(path) = &self.device_paths.nemu_ipc_dll {
            capture.nemu.dll_path = Some(path.clone());
        }
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
        .and_then(|registration| match nemu_app_index {
            Some(index) => registration.with_nemu_app_index(index),
            None => Ok(registration),
        })
        .map_err(|_| "instance_registration_invalid")
    }

    fn fixture_backend(self) -> Result<InstanceSpec, &'static str> {
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
        InstanceSpec::fixture(
            self.alias,
            self.instance_id,
            FixtureInstanceSpec::new(frames, configured.max_inputs),
        )
        .map_err(|_| "fixture_backend_invalid")
    }
}

impl ConfiguredProvider {
    fn new(
        instances: Vec<ConfiguredInstance>,
        mumu_root: Option<PathBuf>,
        vision_manifest: Option<(PathBuf, PathBuf)>,
    ) -> Self {
        let mut specs = Vec::new();
        let mut deferred = Vec::new();
        for instance in instances {
            match instance {
                ConfiguredInstance::Spec(spec) => specs.push(spec),
                ConfiguredInstance::Deferred(entry) => deferred.push(*entry),
            }
        }
        Self {
            instances: specs,
            deferred,
            vision_manifest,
            mumu_root,
        }
    }

    /// The scheduled-execution mode of a configured alias; a discovery-bound instance is a
    /// device. Alias uniqueness is the registry's rule, so the first declaration answers.
    pub(super) fn mode_for_alias(&self, instance_alias: &str) -> Option<ScheduledExecutionMode> {
        self.instances
            .iter()
            .find(|spec| spec.alias() == instance_alias)
            .map(spec_mode)
            .or_else(|| {
                self.deferred
                    .iter()
                    .any(|entry| entry.alias == instance_alias)
                    .then_some(ScheduledExecutionMode::DeviceRegistry)
            })
    }

    /// Every configured alias with its mode (policy bootstrap, `check-config`).
    pub(super) fn modes(&self) -> BTreeMap<String, ScheduledExecutionMode> {
        let mut modes = BTreeMap::new();
        for spec in &self.instances {
            modes
                .entry(spec.alias().to_owned())
                .or_insert_with(|| spec_mode(spec));
        }
        for entry in &self.deferred {
            modes
                .entry(entry.alias.clone())
                .or_insert(ScheduledExecutionMode::DeviceRegistry);
        }
        modes
    }

    /// The binding keys of the instances still waiting for discovery (`check-config`).
    pub(super) fn deferred_bindings(&self) -> BTreeMap<String, InstanceBindingKey> {
        self.deferred
            .iter()
            .map(|entry| (entry.alias.clone(), entry.key.clone()))
            .collect()
    }

    /// The configured `mumu_root`, already validated as absolute (`check-config` reporting).
    pub(super) fn mumu_root(&self) -> Option<&Path> {
        self.mumu_root.as_deref()
    }

    fn instance_count(&self) -> usize {
        self.instances.len() + self.deferred.len()
    }

    /// The registry as configured, before provider startup: no vision provider, and every
    /// discovery-bound instance registered with its stand-in target. `check-config`
    /// validates and reports through it; the daemon registers once, at startup, after
    /// discovery (`assemble_provider`).
    pub(super) fn into_registry(self) -> Result<ExecutionBackendRegistry, RuntimeHostError> {
        let Self {
            mut instances,
            deferred,
            mumu_root,
            ..
        } = self;
        instances.extend(deferred.into_iter().map(|entry| entry.stand_in));
        ExecutionBackendRegistry::from_assembly(ProviderAssembly {
            instances,
            vision: None,
            discovery: Some(DiscoverySpec::new(mumu_root)),
        })
    }
}

fn spec_mode(spec: &InstanceSpec) -> ScheduledExecutionMode {
    match spec.mode() {
        InstanceMode::Real(_) => ScheduledExecutionMode::DeviceRegistry,
        InstanceMode::Fixture(_) => ScheduledExecutionMode::FixtureSimulation,
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
    use actingcommand_contract::{ConfigParameterSource, FactScalar, IdentifierIssuer};
    use actingcommand_device::DeviceError;
    use actingcommand_vision_ffi::{FastDeployPpocrArtifacts, OnnxExecutionProvider};
    use serde_json::json;
    use tempfile::TempDir;

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
        let registry = config
            .assemble()
            .expect("runtime assembly")
            .provider
            .into_registry()
            .expect("configured registry");
        let mut backend = ExecutionBackendProvider::open_input(&registry, "neutral.fixture")
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
            let registry = assembly
                .provider
                .into_registry()
                .expect("configured registry");
            let host = RuntimeHost::start(assembly.host, Arc::new(registry)).expect("runtime host");
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
        for (configured, enabled, source, reason) in [
            (None, true, ConfigParameterSource::Default, "flag absent"),
            (
                Some(false),
                false,
                ConfigParameterSource::Explicit,
                "configured off",
            ),
        ] {
            let mut value = value.clone();
            if let Some(configured) = configured {
                value["frame_retention_enabled"] = json!(configured);
                value["frame_retention_failed_run_successes"] = json!(4);
                value["frame_retention_failed_run_days"] = json!(9);
                // Workflow #318 cfg2: the new tunables ride the same explicit/default toggle.
                value["performance"] = json!({
                    "pressure_start_samples": 5,
                    "pressure_end_samples": 7
                });
                value["device_paths"] = json!({ "nemu_folder": root.path() });
            }
            let config = serde_json::from_value::<ActingdConfigFile>(value).expect("typed config");
            assert_eq!(config.frame_retention_enabled, configured);
            let assembly = config.assemble().expect("runtime assembly");
            let parameter = |key: &str| {
                assembly
                    .manifest
                    .parameters
                    .iter()
                    .find(|parameter| parameter.key == key)
            };
            // cfg2: every reported value is the host's effective one, not a library default.
            let performance_monitor = assembly
                .host
                .performance_monitor()
                .expect("assembled performance monitor");
            for (key, expected, effective) in [
                (
                    "performance_monitor.pressure_start_samples",
                    if configured.is_some() { 5 } else { 3 },
                    performance_monitor.pressure_start_samples(),
                ),
                (
                    "performance_monitor.pressure_end_samples",
                    if configured.is_some() { 7 } else { 3 },
                    performance_monitor.pressure_end_samples(),
                ),
            ] {
                let parameter = parameter(key).expect("pressure streak parameter");
                assert_eq!(parameter.value, FactScalar::Integer(expected));
                assert_eq!(parameter.value, FactScalar::Integer(i64::from(effective)));
                assert_eq!(parameter.source, source);
            }
            let sample_interval = parameter("performance_monitor.sample_interval_ms")
                .expect("sample interval parameter");
            assert_eq!(
                sample_interval.value,
                FactScalar::DurationMs(
                    u64::try_from(performance_monitor.sample_interval().as_millis())
                        .expect("sample interval ms")
                )
            );
            assert_eq!(sample_interval.source, ConfigParameterSource::Default);
            let scheduler = assembly.host.scheduler();
            assert_eq!(
                parameter("scheduler.lease_ttl_ms")
                    .expect("scheduler parameter")
                    .value,
                FactScalar::DurationMs(scheduler.lease_ttl_ms)
            );
            match parameter("device_paths.nemu_folder") {
                Some(nemu_folder) => {
                    assert!(configured.is_some());
                    assert_eq!(
                        nemu_folder.value,
                        FactScalar::String(root.path().display().to_string())
                    );
                    assert_eq!(nemu_folder.source, ConfigParameterSource::Explicit);
                }
                None => assert!(configured.is_none()),
            }
            assert!(
                parameter("device_paths.nemu_ipc_dll").is_none(),
                "an unconfigured device path is omitted, never invented"
            );
            assert_eq!(assembly.host.state_root(), root.path());
            let configuration = assembly
                .provider
                .into_registry()
                .expect("configured registry")
                .resolve("node.a")
                .expect("registered instance")
                .configuration()
                .cloned()
                .expect("effective device configuration");
            assert_eq!(
                configuration.input_backend,
                TouchBackendChoice::MaaTouch.as_str()
            );
            assert_eq!(
                configuration.capture_backend,
                CaptureBackendChoice::Adb.as_str()
            );
            let retention = assembly
                .manifest
                .subsystems
                .iter()
                .find(|subsystem| subsystem.name == "frame_retention")
                .expect("frame retention subsystem");
            assert_eq!(retention.enabled, enabled);
            assert_eq!(retention.reason, reason);
            let parameter = assembly
                .manifest
                .parameters
                .iter()
                .find(|parameter| parameter.key == "frame_retention_enabled")
                .expect("frame retention parameter");
            assert_eq!(parameter.value, FactScalar::Boolean(enabled));
            assert_eq!(parameter.source, source);
            for (key, expected) in [
                (
                    "frame_retention_failed_run_successes",
                    if configured.is_some() { 4 } else { 3 },
                ),
                (
                    "frame_retention_failed_run_days",
                    if configured.is_some() { 9 } else { 7 },
                ),
            ] {
                let parameter = assembly
                    .manifest
                    .parameters
                    .iter()
                    .find(|parameter| parameter.key == key)
                    .expect("failed-run policy parameter");
                assert_eq!(parameter.value, FactScalar::Integer(expected));
                assert_eq!(parameter.source, source);
            }
        }
        for (key, invalid) in [
            ("frame_retention_failed_run_successes", 0),
            ("frame_retention_failed_run_successes", 1025),
            ("frame_retention_failed_run_days", 0),
            ("frame_retention_failed_run_days", 36501),
        ] {
            let mut value = value.clone();
            value[key] = json!(invalid);
            let config = serde_json::from_value::<ActingdConfigFile>(value).expect("typed config");
            assert!(matches!(
                config.assemble(),
                Err("invalid_failed_run_retention_policy")
            ));
        }
        // cfg2: a pressure streak outside 1..=30 and a relative or missing device path are
        // refused at assembly with their own codes.
        for (section, invalid, code) in [
            (
                "performance",
                json!({ "pressure_start_samples": 0 }),
                "invalid_pressure_samples",
            ),
            (
                "performance",
                json!({ "pressure_end_samples": 31 }),
                "invalid_pressure_samples",
            ),
            (
                "device_paths",
                json!({ "minitouch_path": "external-tools/minitouch/minitouch" }),
                "device_path_invalid",
            ),
            (
                "device_paths",
                json!({ "droidcast_apk": root.path().join("missing.apk") }),
                "device_path_invalid",
            ),
        ] {
            let mut value = value.clone();
            value[section] = invalid;
            let config = serde_json::from_value::<ActingdConfigFile>(value).expect("typed config");
            assert_eq!(config.assemble().err(), Some(code));
        }
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
            assembly.provider.mode_for_alias("neutral.fixture"),
            Some(ScheduledExecutionMode::FixtureSimulation)
        );
        assert!(
            assembly
                .provider
                .into_registry()
                .expect("configured registry")
                .vision_provider()
                .is_none()
        );

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
                assembly.provider.assemble_provider(startup)
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
            assembly.provider.mode_for_alias("neutral.device"),
            Some(ScheduledExecutionMode::DeviceRegistry)
        );
        assert_eq!(
            assembly.provider.mode_for_alias("neutral.fixture"),
            Some(ScheduledExecutionMode::FixtureSimulation)
        );
        let registry = assembly
            .provider
            .into_registry()
            .expect("configured registry");
        assert_eq!(registry.instance_aliases().len(), 2);
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
                assembly.provider.mode_for_alias(&alias),
                Some(ScheduledExecutionMode::DeviceRegistry)
            );
            assert_eq!(
                assembly.provider.mode_for_alias(" Fixture Ω "),
                Some(ScheduledExecutionMode::FixtureSimulation)
            );
            assert!(
                assembly
                    .provider
                    .mode_for_alias("unknown.instance")
                    .is_none()
            );
            let registry = assembly
                .provider
                .into_registry()
                .expect("configured registry");
            assert_eq!(
                registry.resolve(&alias).unwrap().instance_id(),
                *device_id.transport()
            );
            assert!(registry.resolve(" fixture Ω ").is_none());
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
