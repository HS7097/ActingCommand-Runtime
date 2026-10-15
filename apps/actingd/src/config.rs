// SPDX-License-Identifier: AGPL-3.0-only

use crate::codes::ActingdCode;
use crate::monitor_retry::{self, PolicyMonitorSettings};
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::{Code, Outcome};
use actingcommand_contract::resource_declaration::{
    ProcedureBindingConfigFile, ScheduledExecutionConfigFile,
};
use actingcommand_contract::{
    ApprovalDecisionRecord, ApprovalDisposition, ApprovalTarget, ContainedTaskRequest, InstanceId,
    InstanceResourcePackage, InstanceResourcePackageKind, RuntimeConfigManifest,
};
use actingcommand_device::{
    AdbConfig, CaptureBackendChoice, CaptureBackendConfig, CaptureBackendName, DeviceKind,
    DeviceSerial, DeviceTarget, EnvOverrides, Frame, MaaTouchConfig, MinitouchConfig, PixelFormat,
    TouchBackendChoice, TouchBackendConfig,
};
use actingcommand_execution_kernel::PreparedContainedTask;
use actingcommand_policy::{
    CatalogDocumentSource, CatalogSources, EvaluationFacts, EvaluationResources, MAX_APPROVAL_REFS,
    MAX_CATALOG_BYTES, MAX_DOCUMENT_BYTES, MAX_REFERENCES_PER_TASK, MAX_TASKS, compile_catalog,
};
use actingcommand_runtime_config::performance::{monitor, pacing};
use actingcommand_runtime_host::{
    AgentDispatcherConfig, CatalogTransitionRequest, DiscoverySpec, ExecutionBackendProvider,
    ExecutionBackendRegistration, ExecutionBackendRegistry, FixtureInstanceSpec, GovernancePolicy,
    HandsetEndpoint, InstanceMode, InstanceSpec, PerformanceControlConfig,
    PerformanceMonitorConfig, PolicyCadence, PolicyInputSnapshot, ProcedureBinding,
    ProcedureManifest, ProviderAssembly, RecognitionVisionProvider, RuntimeHostConfig,
    RuntimeHostError, VisionFfiProvider, VisionSpec,
};
use actingcommand_vision_ffi::{
    CudaDeviceSelector, OnnxExecutionProvider, VISION_MODELS_DIRECTORY, VisionListingCode,
    VisionModelListing, VisionRuntimeSpec, list_vision_models,
};
use serde::{Deserialize, Deserializer};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub(super) mod ac_adb;
mod device_binding;
// Workflow #384 D2-S2.
pub(super) mod recognition_package;
pub(crate) use device_binding::DeviceRefusal;
mod manifest;
#[cfg(test)]
pub(crate) use manifest::env_overrides_subsystem;
mod provider_startup;
pub(crate) use provider_startup::device_failure_outcome;
// Test-only: Workflow #382 A-6 (test plan v2.2 §4 #13), the start's configuration refusals.
#[cfg(test)]
mod gate_382_a6;

const CONFIG_SCHEMA_VERSION: &str = "actingcommand.actingd.config.v1";
const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
const MAX_TIMEOUT_MS: u64 = 120_000;
const MAX_FIXTURE_FRAMES: usize = 32;
const MAX_FIXTURE_FRAME_BYTES: usize = 16 * 1024 * 1024;
const MAX_FIXTURE_RESIDENT_BYTES: usize = 32 * 1024 * 1024;
const MAX_FIXTURE_INPUTS: u16 = 32;
/// Workflow #318 cfg4: the most `governance.allowed_clients` entries a file may name.
const MAX_GOVERNANCE_ALLOWED_CLIENTS: usize = 32;
/// Workflow #288: the admission deadline of one digest-named `resource_package` directory.
const RESOURCE_PACKAGE_DIRECTORY_DEADLINE: Duration = Duration::from_secs(60);
/// Workflow #360: the vision root's default name under the install root or the configuration
/// file's directory.
const DEFAULT_VISION_ROOT: &str = "vision";
/// The governance identity card `client` of the daemon's own policy driver connection; it is
/// always allowed, whatever `governance.allowed_clients` names.
pub(super) const GOVERNANCE_POLICY_DRIVER_CLIENT: &str = "actingd-policy-driver";

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
    /// Workflow #375 R5c: whether each 30 s error window drops its interior near-duplicates
    /// (default `true`). Ordinary frames are always deduplicated and have no key.
    // Interface anchor `actingd-config` revision 4 (contracts/component-interfaces.md): a Runtime
    // up to v0.11.4 refuses a file that names either dedup switch; one that names neither keeps
    // its revision. Raise the revision in distribution/windows/component-interfaces.json.
    #[serde(default)]
    frame_retention_dedup_error: Option<bool>,
    /// Workflow #375 R5c: whether Lab output drops its interior near-duplicates (default
    /// `false`); Lab operation evidence is never deduplicated.
    #[serde(default)]
    frame_retention_dedup_lab: Option<bool>,
    /// Workflow #389 U-2 (model §4.1): the Lab's two options, option one (authoring) and option
    /// two (debugging); absent means not installed. Read and reported only until U-6
    /// (`crates/runtime-config`, `lab_module`); only the installer's Lab transaction writes them.
    // Interface anchor `actingd-config` revision 5 (contracts/component-interfaces.md): a Runtime
    // before v0.12.0 refuses a file that names either key; one that names neither keeps its
    // revision.
    #[serde(default)]
    lab_module_authoring: Option<bool>,
    #[serde(default)]
    lab_module_debugging: Option<bool>,
    /// Workflow #318 cfg4: the retired shared governance secret. Kept only so a file that
    /// still names the key, whatever its value, is refused with the precise
    /// `governance_capability_retired` instead of the generic decode failure.
    #[serde(
        default,
        rename = "governance_capability",
        deserialize_with = "retired_key_present"
    )]
    governance_capability_retired: bool,
    /// Workflow #318 cfg4: which governance identity cards the daemon accepts.
    #[serde(default)]
    governance: Option<GovernanceConfigFile>,
    #[serde(default)]
    agent_dispatcher: Option<AgentDispatcherConfigFile>,
    #[serde(default)]
    policy: Option<PolicyConfigFile>,
    /// Workflow #360: the retired v0.3 provider manifest. Kept only so a file that still
    /// names the key, whatever its value, is refused with `vision_provider_manifest_retired`;
    /// the `vision` section replaces it.
    // Interface anchor `actingd-config` (contracts/component-interfaces.md): raise its revision
    // in distribution/windows/component-interfaces.json when older writers' files are refused.
    #[serde(
        default,
        rename = "vision_provider_manifest",
        deserialize_with = "retired_key_present"
    )]
    vision_provider_manifest_retired: bool,
    /// Workflow #360: the vision root and execution provider; absent means no vision.
    #[serde(default)]
    vision: Option<VisionConfigFile>,
    /// Explicit MuMu install root: the highest-priority `MuMuManager.exe` discovery source.
    #[serde(default)]
    mumu_root: Option<PathBuf>,
    /// Workflow #318 (cfg2): the pressure streaks; Workflow #385 P-c: the whole `performance`
    /// section (`crates/runtime-config`), the streaks keeping their members, each member
    /// optional; the defaults file holds every default.
    #[serde(default)]
    performance: Option<actingcommand_runtime_config::PerformanceFile>,
    /// Workflow #318 (cfg2): daemon-level device tool paths, applied to every device
    /// instance's backend configuration; absent fields keep today's env / discovery /
    /// bundled-tool behaviour.
    #[serde(default)]
    device_paths: Option<DevicePathsConfigFile>,
    /// Workflow #318 (cfg3): whether the `ACTINGCOMMAND_*` environment fallbacks
    /// (`EnvOverrides::VARIABLES`) are read at all (default `false`). Off, a set variable is
    /// ignored and named as `env_override_ignored:<VAR>`, never silently used.
    #[serde(default)]
    allow_env_overrides: Option<bool>,
    /// Workflow #373 NAV-6: the installed standard packs (`standard_packs.root`), read at
    /// startup. Workflow #387 D1c: the section's file is `crates/runtime-config`'s, and the key
    /// is reported in `config.parameters.standard_packs`. Workflow #373 NAV-3a (M-373 §4.3):
    /// every prerequisite and return-home package is found in its registries, so the retired
    /// `prerequisite_packages` and `return_home_packages` are unknown members
    /// (`config_decode_failed`).
    #[serde(default)]
    standard_packs: Option<actingcommand_runtime_config::StandardPacksFile>,
    /// Workflow #387 D1c: the recovery ladder (`recovery_ladder.*`) and the Runtime-held keys
    /// (`host_claims.*`), each member optional; the defaults file holds every default.
    #[serde(default)]
    recovery_ladder: Option<actingcommand_runtime_config::RecoveryLadderFile>,
    #[serde(default)]
    host_claims: Option<actingcommand_runtime_config::HostClaimsFile>,
    /// Workflow #386 SR-1: the scheduling rules (`scheduling.*`), each member optional; the
    /// defaults file holds every default.
    #[serde(default)]
    scheduling: Option<actingcommand_runtime_config::SchedulingFile>,
    /// Workflow #383 AO-1: the ledger observer (`ledger_observer.*`, `contracts/ledger-observer.md`
    /// §15), each member optional; the defaults file holds every default.
    #[serde(default)]
    ledger_observer: Option<actingcommand_runtime_config::LedgerObserverFile>,
    /// Workflow #398 RD-2: the wired device link of a handset (`device_link.*`, model-398 §4.6),
    /// each member optional; the defaults file holds every default.
    #[serde(default)]
    device_link: Option<actingcommand_runtime_config::DeviceLinkFile>,
    /// Workflow #398 RD-5b: a handset's device state (`device_state.*`, model-398 §7.8), each
    /// member optional; the defaults file holds every default.
    #[serde(default)]
    device_state: Option<actingcommand_runtime_config::DeviceStateFile>,
    instances: Vec<InstanceConfig>,
    #[serde(skip)]
    source_root: PathBuf,
    /// Workflow #387 D1b (`config.selection`): the file's `sha256:<hex>` as `load_for` read it
    /// and, for an installed Runtime, the configuration generation acsetup committed.
    #[serde(skip)]
    config_sha256: Option<String>,
    #[serde(skip)]
    config_generation: Option<u64>,
    /// The install root the daemon runs from, when it runs from one (set by `load_for`).
    #[serde(skip)]
    install_root: Option<PathBuf>,
}

/// Workflow #387 D1c: one instance's stuck-recovery settings from the rows the configuration
/// mechanism resolved: its `stuck_recovery` and `stuck_recovery_cooldown_secs`, which take the
/// `recovery_ladder` section's values when the file names neither.
fn stuck_recovery_settings(
    parameters: &[actingcommand_contract::ConfigParameter],
    instance_id: &str,
) -> Result<actingcommand_contract::InstanceStuckRecovery, Code> {
    let value = |member: &str| {
        let key = actingcommand_runtime_config::instance_key(instance_id, member);
        parameters
            .iter()
            .find(|parameter| parameter.key == key)
            .map(|parameter| parameter.value.clone())
    };
    match (
        value(actingcommand_runtime_config::STUCK_RECOVERY),
        value(actingcommand_runtime_config::STUCK_RECOVERY_COOLDOWN_SECS),
    ) {
        (
            Some(actingcommand_contract::FactScalar::Boolean(enabled)),
            Some(actingcommand_contract::FactScalar::Integer(cooldown_secs)),
        ) => Ok(actingcommand_contract::InstanceStuckRecovery {
            enabled,
            cooldown_secs: u32::try_from(cooldown_secs)
                .map_err(|_| ActingdCode::StuckRecoveryCooldownInvalid.code())?,
        }),
        _ => Err(ActingdCode::ConfigManifestInvalid.code()),
    }
}

/// Workflow #385 P-c: the values of the `performance` section the start-up assembly reads
/// (PERF §10.1, §10.3), from the rows the configuration mechanism resolved: every key of the
/// monitor's row (PM-CFG: through runtime-config; a value that breaks a rule is refused there,
/// naming its key, into `refused`), and the controller's clock jump and heavy-dispatch limits,
/// which the host took from code constants until now.
fn performance_settings(
    parameters: &[actingcommand_contract::ConfigParameter],
    refused: &mut Option<Outcome>,
) -> Result<(PerformanceMonitorConfig, PerformanceControlConfig), Code> {
    let value = |key: &str| {
        parameters
            .iter()
            .find(|parameter| parameter.key == key)
            .map(|parameter| &parameter.value)
    };
    let small = |key: &str| match value(key) {
        Some(actingcommand_contract::FactScalar::Integer(number)) => {
            u16::try_from(*number).map_err(|_| ActingdCode::ConfigManifestInvalid.code())
        }
        _ => Err(ActingdCode::ConfigManifestInvalid.code()),
    };
    let clock_jump = match value(pacing::CLOCK_JUMP_MS) {
        Some(actingcommand_contract::FactScalar::DurationMs(ms)) => Duration::from_millis(*ms),
        _ => return Err(ActingdCode::ConfigManifestInvalid.code()),
    };
    // Workflow #385 PM-CFG (PERF §10.1; register §5 "Read by P-m"): the monitor, its sampler
    // and the controller read every key of the row in effect.
    let monitor =
        monitor::MonitorSettings::parse(&|key: &str| value(key).cloned()).map_err(|failure| {
            *refused = Some(failure.outcome());
            failure.failure_code().code()
        })?;
    let monitor_config = PerformanceMonitorConfig::default().with_settings(&monitor);
    let control_config = PerformanceControlConfig::default()
        .with_clock_jump_threshold(clock_jump)
        .with_heavy_dispatch_limits(
            small(pacing::HEAVY_DISPATCH_LIMIT_NORMAL)?,
            small(pacing::HEAVY_DISPATCH_LIMIT_PAUSED)?,
        )
        .with_heavy_threshold_milli(monitor.heavy_threshold_milli);
    Ok((monitor_config, control_config))
}

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
    fn validate(&self) -> Result<(), Code> {
        for (_, path) in self.entries() {
            if let Some(path) = path
                && (path.as_os_str().is_empty()
                    || !path.is_absolute()
                    || fs::metadata(path).is_err())
            {
                return Err(ActingdCode::DevicePathInvalid.code());
            }
        }
        Ok(())
    }
}

/// `governance { allowed_clients }` (Workflow #318 cfg4): the identity card `client` names
/// the daemon accepts, each following the card's rule (`1..=64` bytes of `[A-Za-z0-9._-]`),
/// at most 32, no duplicates. Without the section any well-formed card is accepted.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GovernanceConfigFile {
    allowed_clients: Vec<String>,
}

impl GovernanceConfigFile {
    /// The host policy: the named clients plus the daemon's own policy driver.
    fn policy(&self) -> Result<GovernancePolicy, Code> {
        let mut allowed = BTreeSet::new();
        if self.allowed_clients.len() > MAX_GOVERNANCE_ALLOWED_CLIENTS {
            return Err(ActingdCode::GovernanceAllowedClientsInvalid.code());
        }
        for client in &self.allowed_clients {
            if actingcommand_contract::validate_governance_client(client).is_err()
                || !allowed.insert(client.clone())
            {
                return Err(ActingdCode::GovernanceAllowedClientsInvalid.code());
            }
        }
        allowed.insert(GOVERNANCE_POLICY_DRIVER_CLIENT.to_owned());
        Ok(GovernancePolicy {
            allowed_clients: Some(allowed),
        })
    }
}

/// Present whatever the value: only called by serde when the key is in the file.
fn retired_key_present<'de, D: Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    serde::de::IgnoredAny::deserialize(deserializer)?;
    Ok(true)
}

/// Workflow #360: the `vision` section. The vision root holds `ort\` (ONNX Runtime) and
/// `models\<name>\`, one folder per model; models are listed at startup and each is read,
/// hashed and loaded on its first use.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct VisionConfigFile {
    /// `cpu` or `cuda`, for every OCR and NN model of the process.
    execution_provider: OnnxExecutionProvider,
    /// Required for `cuda`, refused for `cpu`.
    #[serde(default)]
    cuda_device: Option<CudaDeviceSelector>,
    /// Overrides the default root (`<install root>\vision`, else `<config dir>\vision`); a
    /// relative path resolves against the configuration file's directory.
    #[serde(default)]
    root: Option<PathBuf>,
}

impl VisionConfigFile {
    fn assemble(
        self,
        source_root: &Path,
        install_root: Option<&Path>,
    ) -> Result<ConfiguredVision, Code> {
        match (self.execution_provider, &self.cuda_device) {
            (OnnxExecutionProvider::Cpu, None) => {}
            (OnnxExecutionProvider::Cuda, Some(selector)) if selector.validate().is_ok() => {}
            _ => return Err(ActingdCode::VisionConfigInvalid.code()),
        }
        let (configured, base) = match self.root {
            Some(root) if root.as_os_str().is_empty() => {
                return Err(ActingdCode::VisionConfigInvalid.code());
            }
            Some(root) => (root, source_root.to_path_buf()),
            None => (
                PathBuf::from(DEFAULT_VISION_ROOT),
                install_root.unwrap_or(source_root).to_path_buf(),
            ),
        };
        Ok(ConfiguredVision {
            configured,
            base,
            install_root: install_root.map(Path::to_path_buf),
            execution_provider: self.execution_provider,
            cuda_device: self.cuda_device,
        })
    }
}

/// The assembled `vision` section: where the vision root is and how it was chosen. Nothing
/// is read until provider startup or `check-config` inspects it.
pub(super) struct ConfiguredVision {
    /// The configured root, or `vision` for the default.
    configured: PathBuf,
    /// What a relative `configured` resolves against: the configuration file's directory, or
    /// the install root for the default when the daemon runs from one.
    base: PathBuf,
    /// The install root; a vision root inside one of its program slots is refused.
    install_root: Option<PathBuf>,
    execution_provider: OnnxExecutionProvider,
    cuda_device: Option<CudaDeviceSelector>,
}

/// What one inspection of the vision root found: the cheap listing startup and
/// `check-config` share. No model file is read and no library is loaded.
pub(super) struct VisionInventory {
    /// The canonical vision root.
    pub(super) root: PathBuf,
    pub(super) models_dir: PathBuf,
    pub(super) runtime: VisionRuntimeSpec,
    pub(super) listing: VisionModelListing,
}

/// Why a vision root cannot be used: the classification and its message.
pub(super) struct VisionRefusal {
    pub(super) code: Code,
    pub(super) message: String,
    /// The foreign failure behind the refusal, when the OS reported one (ledger revision 3:
    /// the provider startup record carries it as the refusal's `caused_by` link).
    pub(super) cause: Option<Outcome>,
}

impl ConfiguredVision {
    pub(super) fn configured(&self) -> &Path {
        &self.configured
    }

    pub(super) fn base(&self) -> &Path {
        &self.base
    }

    pub(super) const fn execution_provider(&self) -> OnnxExecutionProvider {
        self.execution_provider
    }

    /// Lists the vision root: the root must exist outside the install root's program slots,
    /// `models\` must be readable and hold at least one folder, and `ort\onnxruntime.dll` must
    /// be present (with the CUDA provider library for `cuda`). A folder that breaks the
    /// folder rule is listed as invalid, not refused.
    pub(super) fn inspect(&self) -> Result<VisionInventory, VisionRefusal> {
        let configured = if self.configured.is_absolute() {
            self.configured.clone()
        } else {
            self.base.join(&self.configured)
        };
        let root = fs::canonicalize(&configured).map_err(|error| VisionRefusal {
            code: VisionListingCode::VisionRootUnavailable.code(),
            message: format!(
                "vision root {} is unavailable: {error}",
                configured.display()
            ),
            cause: Some(Outcome::from_io_error(
                &error,
                actingcommand_contract::outcome::IoOp::Canonicalize,
            )),
        })?;
        if let Some(install_root) = &self.install_root
            && ["A", "B"]
                .into_iter()
                .any(|slot| root.starts_with(install_root.join(slot)))
        {
            return Err(VisionRefusal {
                code: ActingdCode::VisionRootInProgramSlot.code(),
                message: format!(
                    "vision root {} lies inside a program slot of the install root {}; keep it outside A and B",
                    root.display(),
                    install_root.display()
                ),
                cause: None,
            });
        }
        let models_dir = root.join(VISION_MODELS_DIRECTORY);
        let listing = list_vision_models(&models_dir).map_err(|error| VisionRefusal {
            code: VisionListingCode::VisionRootUnavailable.code(),
            message: error.message().to_owned(),
            cause: error.os_cause().cloned(),
        })?;
        let runtime = VisionRuntimeSpec::from_vision_root(
            &root,
            self.execution_provider,
            self.cuda_device.clone(),
        )
        .map_err(|error| VisionRefusal {
            code: ActingdCode::VisionRuntimeUnavailable.code(),
            message: error.message().to_owned(),
            cause: error.os_cause().cloned(),
        })?;
        if listing.ocr.is_empty() && listing.nn.is_empty() && listing.invalid.is_empty() {
            return Err(VisionRefusal {
                code: VisionListingCode::VisionModelsEmpty.code(),
                message: format!("{} holds no model folder", models_dir.display()),
                cause: None,
            });
        }
        Ok(VisionInventory {
            root,
            models_dir,
            runtime,
            listing,
        })
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
    /// Workflow #361 A: the explicit transition to another catalog id or to a generation that
    /// was active before; absent means the forward-only rule.
    // Interface anchor `actingd-config` revision 3 (contracts/component-interfaces.md): raise its
    // revision in distribution/windows/component-interfaces.json when older readers refuse this.
    #[serde(default)]
    catalog_transition: Option<CatalogTransitionConfigFile>,
    /// Workflow #397: the policy monitor's connection to its Runtime and its retry of a
    /// transient request failure; absent means every default.
    // Interface anchor `actingd-config` revision 5 (contracts/component-interfaces.md): an
    // earlier Runtime refuses a file that names it, because `policy` denies unknown fields.
    #[serde(default)]
    monitor: Option<PolicyMonitorConfigFile>,
}

/// `policy.monitor` (Workflow #397). Each field is optional; an absent one is its default
/// (`monitor_retry::DEFAULT_*`), and an absent `retry_backoff_max_ms` is at least
/// `retry_backoff_ms`.
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct PolicyMonitorConfigFile {
    #[serde(default)]
    io_timeout_ms: Option<u64>,
    #[serde(default)]
    retry_limit: Option<u16>,
    #[serde(default)]
    retry_backoff_ms: Option<u64>,
    #[serde(default)]
    retry_backoff_max_ms: Option<u64>,
}

/// Workflow #397: the bounds of `policy.monitor`. The I/O timeout stays inside the runtime
/// client's own bound (60 s); a wait between two retries is at most 10 min.
const MIN_POLICY_MONITOR_IO_TIMEOUT_MS: u64 = 1_000;
const MAX_POLICY_MONITOR_IO_TIMEOUT_MS: u64 = 60_000;
const MAX_POLICY_MONITOR_RETRY_LIMIT: u16 = 100;
const MAX_POLICY_MONITOR_BACKOFF_MS: u64 = 600_000;

impl PolicyMonitorConfigFile {
    /// The effective settings and which of them the file named; a value outside its bounds is
    /// `policy_monitor_config_invalid`.
    fn parameters(&self) -> Result<manifest::PolicyMonitorParameters, Code> {
        let io_timeout_ms = self
            .io_timeout_ms
            .unwrap_or(monitor_retry::DEFAULT_IO_TIMEOUT_MS);
        let retry_limit = self
            .retry_limit
            .unwrap_or(monitor_retry::DEFAULT_RETRY_LIMIT);
        let retry_backoff_ms = self
            .retry_backoff_ms
            .unwrap_or(monitor_retry::DEFAULT_RETRY_BACKOFF_MS);
        let retry_backoff_max_ms = self
            .retry_backoff_max_ms
            .unwrap_or(monitor_retry::DEFAULT_RETRY_BACKOFF_MAX_MS.max(retry_backoff_ms));
        if !(MIN_POLICY_MONITOR_IO_TIMEOUT_MS..=MAX_POLICY_MONITOR_IO_TIMEOUT_MS)
            .contains(&io_timeout_ms)
            || retry_limit > MAX_POLICY_MONITOR_RETRY_LIMIT
            || !(1..=MAX_POLICY_MONITOR_BACKOFF_MS).contains(&retry_backoff_ms)
            || !(retry_backoff_ms..=MAX_POLICY_MONITOR_BACKOFF_MS).contains(&retry_backoff_max_ms)
        {
            return Err(ActingdCode::PolicyMonitorConfigInvalid.code());
        }
        Ok(manifest::PolicyMonitorParameters {
            settings: PolicyMonitorSettings {
                io_timeout: Duration::from_millis(io_timeout_ms),
                retry_limit,
                retry_backoff: Duration::from_millis(retry_backoff_ms),
                retry_backoff_max: Duration::from_millis(retry_backoff_max_ms),
            },
            io_timeout_explicit: self.io_timeout_ms.is_some(),
            retry_limit_explicit: self.retry_limit.is_some(),
            retry_backoff_explicit: self.retry_backoff_ms.is_some(),
            retry_backoff_max_explicit: self.retry_backoff_max_ms.is_some(),
        })
    }
}

/// `policy.catalog_transition` (Workflow #361 A): `kind` is `replace`, and the transition is
/// applied only while `expected_active_catalog_hash` is the active generation.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogTransitionConfigFile {
    kind: String,
    expected_active_catalog_hash: String,
}

impl CatalogTransitionConfigFile {
    fn request(&self) -> Result<CatalogTransitionRequest, Code> {
        if self.kind != "replace" {
            return Err(ActingdCode::CatalogTransitionKindUnknown.code());
        }
        let canonical = self
            .expected_active_catalog_hash
            .strip_prefix("sha256:")
            .is_some_and(|digest| {
                digest.len() == 64
                    && digest
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
            });
        if !canonical {
            return Err(ActingdCode::CatalogTransitionInvalid.code());
        }
        Ok(CatalogTransitionRequest::replace(
            self.expected_active_catalog_hash.clone(),
        ))
    }
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
    /// Workflow #384 D2-S2 (D2 model §3.2, v1.4.2): the instance's recognition maintenance
    /// package, `{package}`: a location only, never a digest. A relative path resolves against
    /// the configuration file's directory; nothing is opened at assembly.
    // Interface anchor `actingd-config` revision 5 (contracts/component-interfaces.md): an
    // earlier Runtime refuses a file that names it; A8 declares the revision once.
    #[serde(default)]
    recognition_package: Option<RecognitionPackageConfigFile>,
    /// Slice #316-B4: `false` turns the stuck-recovery ladder off for this instance
    /// (default `true`).
    #[serde(default)]
    stuck_recovery: Option<bool>,
    /// Slice #316-B4: at most one stuck-recovery ladder per this many seconds (default 600,
    /// `1..=86400`).
    #[serde(default)]
    stuck_recovery_cooldown_secs: Option<u32>,
    /// Workflow #405 (parsed by #386 SR-1): the instance's own random delay
    /// (`scheduling.random_delay.*`); a member it leaves out is the `scheduling` section's.
    #[serde(default)]
    scheduling: Option<actingcommand_runtime_config::InstanceSchedulingFile>,
    /// Workflow #369 S6a (ORD §3.5 item 17): `true` lets the daemon start this instance's
    /// emulator when it is stopped at daemon start (S6c); discovery-bound instances only
    /// (`start_emulator_requires_discovery_binding`); absent is off.
    #[serde(default)]
    start_emulator: Option<bool>,
    #[serde(default)]
    fixture_backend: Option<FixtureBackendConfigFile>,
    /// Workflow #398 RD-1b (model-398 v1.1 §3.1): the kind of device the instance drives,
    /// `{kind, transport}`; absent is an emulator.
    #[serde(default)]
    device: Option<device_binding::InstanceDeviceConfigFile>,
    /// Workflow #398 RD-1b: a handset's device from the private binding file, bound or unbound,
    /// set by `assemble`; `None` for an emulator.
    #[serde(skip)]
    handset: Option<HandsetEndpoint>,
    /// The daemon-level `device_paths`, copied in by `assemble` so a deferred instance
    /// registered after discovery applies the same paths as an explicit one.
    #[serde(skip)]
    device_paths: DevicePathsConfigFile,
    /// The injected environment fallbacks (empty unless `allow_env_overrides`), copied in by
    /// `assemble` like `device_paths`.
    #[serde(skip)]
    env_overrides: EnvOverrides,
    /// Workflow #337: the install root's adb and whether this instance uses it (`adb_path`
    /// absent or naming that file), resolved once by `assemble` like `device_paths`.
    #[serde(skip)]
    adb: ac_adb::InstanceAdb,
}

/// Same semantics as `actingctl task-run --package <locator>`: the locator (relative paths
/// resolve against the configuration file's directory). Workflow #336 PKG-S1/PKG-S4 (option
/// 丙): the locator's path chooses the loader and no name is read as an identity; the entry is
/// `{package}` only, and one that still names `expected_sha256` does not decode
/// (`config_decode_failed`). Nothing is opened or hashed at assembly.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct StartupPackageConfigFile {
    package: PathBuf,
}

impl StartupPackageConfigFile {
    fn request(self, source_root: &Path) -> Result<ContainedTaskRequest, Code> {
        let path = if self.package.is_absolute() {
            self.package
        } else {
            source_root.join(self.package)
        };
        if !path.is_absolute() {
            return Err(ActingdCode::StartupPackagePathInvalid.code());
        }
        ContainedTaskRequest::new(path.to_string_lossy().into_owned())
            .and_then(|request| {
                request.with_response_deadline_ms(ContainedTaskRequest::MAX_RESPONSE_DEADLINE_MS)
            })
            .map_err(|_| ActingdCode::StartupPackageInvalid.code())
    }
}

/// Workflow #384 D2-S2: `instances[].recognition_package`, exactly `{package}` (M-PKG §4.5).
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecognitionPackageConfigFile {
    package: PathBuf,
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
    /// Workflow #387 D1b: the configuration layers read, handed to `host` as `config.selection`;
    /// `check-config` prints it. `None` only for a file `load_for` did not read.
    pub(super) config_selection: Option<actingcommand_contract::ConfigSelection>,
    /// Per instance alias: the configured `resource_package`, resolved but not yet admitted.
    pub(super) resource_packages: BTreeMap<String, PathBuf>,
    /// Workflow #384 D2-S2: per instance alias, the configured `recognition_package` location,
    /// resolved but not opened.
    pub(super) recognition_packages: BTreeMap<String, PathBuf>,
    /// The `ACTINGCOMMAND_*` variables that are set but ignored because
    /// `allow_env_overrides` is off, in `EnvOverrides::VARIABLES` order.
    pub(super) ignored_env_overrides: Vec<&'static str>,
    /// Workflow #337: the install root's adb and the instances that use it, checked by
    /// `ac_adb::require` before any side effect. Internal; never a configuration field.
    pub(super) adb_requirement: ac_adb::AdbRequirement,
    /// Workflow #398 RD-1b: the handset instances the private binding file binds no device to,
    /// in configuration order: registered unbound at a start, refused by `check-config`.
    pub(super) unbound_handsets: Vec<String>,
    /// Workflow #398 RD-1b: the aliases the private binding file binds that no handset instance
    /// of the configuration has, in file order (`check-config` notes).
    pub(super) unused_device_bindings: Vec<String>,
    /// Workflow #369 S6a: the discovery-bound instances whose `start_emulator` is `true`, in
    /// configuration order: the daemon starts their stopped emulators at its start (S6c);
    /// `check-config` reports each instance's value.
    pub(super) emulator_autostart: Vec<String>,
}

/// A refused `resource_package`: the code, the offending instance and path and, for
/// `resource_package_invalid`, the package loader's own code and message.
pub(super) struct ResourcePackageRejection {
    pub(super) code: Code,
    alias: String,
    path: PathBuf,
    loader: Option<(&'static str, String)>,
}

impl ResourcePackageRejection {
    /// Workflow #378 A6a (model section 7.4): the refusal with its `alias` and `path`, and the
    /// package loader's code as its `caused_by` link; the loader's message is developer text.
    pub(super) fn outcome(&self) -> Outcome {
        let outcome = Outcome::new(self.code).alias(&self.alias).path(&self.path);
        match &self.loader {
            Some((code, _)) => outcome.caused_by(Outcome::relayed(code)),
            None => outcome,
        }
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
/// startup: the path must exist (`resource_package_missing`); a directory or a file must be
/// admitted by the contained-task package loader exactly as `task-run` admits one, by its path
/// and structurally (`resource_package_invalid`). Workflow #336 PKG-S1: no digest is computed
/// or compared and no name is read as an identity; the field carries no reference, so none is
/// recorded.
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
        let path = std::path::absolute(configured_path).map_err(|_| {
            rejected(
                ActingdCode::ResourcePackageMissing.code(),
                configured_path,
                None,
            )
        })?;
        let metadata = fs::metadata(&path)
            .map_err(|_| rejected(ActingdCode::ResourcePackageMissing.code(), &path, None))?;
        let invalid = |code, message: String| {
            rejected(
                ActingdCode::ResourcePackageInvalid.code(),
                &path,
                Some((code, message)),
            )
        };
        let loader_refused = |error: actingcommand_execution_kernel::ContainedTaskError| {
            invalid(
                error.code().as_str(),
                error
                    .detail()
                    .map_or_else(|| error.to_string(), str::to_owned),
            )
        };
        let kind = if metadata.is_dir() {
            InstanceResourcePackageKind::Directory
        } else if metadata.is_file() {
            InstanceResourcePackageKind::File
        } else {
            return Err(rejected(
                ActingdCode::ResourcePackageInvalid.code(),
                &path,
                None,
            ));
        };
        PreparedContainedTask::load_path(
            alias,
            &path,
            None,
            Instant::now() + RESOURCE_PACKAGE_DIRECTORY_DEADLINE,
        )
        .map_err(loader_refused)?;
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
    pub(super) catalog_approval_ids: Vec<String>,
    pub(super) catalog: CatalogSources,
    /// Workflow #361 A: the configured `replace` transition, if any.
    pub(super) catalog_transition: Option<CatalogTransitionRequest>,
    pub(super) scheduled_tasks: BTreeMap<String, ScheduledProcedureTask>,
    pub(super) registry_modes: BTreeMap<String, ScheduledExecutionMode>,
    pub(super) cadence: PolicyCadence,
}

/// The parsed provider configuration: the typed instance specs the registry consumes plus
/// what provider startup still resolves (discovery-bound instances, the vision root).
/// `assemble` registers nothing and spawns nothing; `ExecutionBackendRegistry::from_assembly`
/// is the one place instances are registered.
pub(super) struct ConfiguredProvider {
    /// Explicit device and fixture instances.
    instances: Vec<InstanceSpec>,
    /// Instances bound by `instance_index`/`instance_name` after one `MuMuManager` discovery
    /// run at provider startup.
    deferred: Vec<DeferredInstance>,
    /// The `vision` section, listed at startup (Workflow #360).
    vision: Option<ConfiguredVision>,
    mumu_root: Option<PathBuf>,
    /// The injected environment fallbacks for discovery and the vision provider.
    env_overrides: EnvOverrides,
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

pub(super) fn load(path: &Path) -> Result<ActingdConfigFile, Code> {
    load_for(path, actingcommand_contract::InstallConfigPurpose::Running)
}

pub(super) fn load_for(
    path: &Path,
    purpose: actingcommand_contract::InstallConfigPurpose,
) -> Result<ActingdConfigFile, Code> {
    let installation =
        actingcommand_contract::process_installation_for(purpose).map_err(|error| error.code())?;
    let selected_path = installation
        .map(|selected| selected.resolve_config(path))
        .transpose()
        .map_err(|error| error.code())?;
    let path = selected_path.as_deref().unwrap_or(path);
    let metadata = fs::metadata(path).map_err(|_| ActingdCode::ConfigUnavailable.code())?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_CONFIG_BYTES {
        return Err(ActingdCode::ConfigSizeInvalid.code());
    }
    let bytes = match installation {
        Some(selected) => selected.config_bytes().map_err(|error| error.code())?,
        None => fs::read(path).map_err(|_| SharedCode::ConfigReadFailed.code())?,
    };
    let mut config = serde_json::from_slice::<ActingdConfigFile>(&bytes)
        .map_err(|_| SharedCode::ConfigDecodeFailed.code())?;
    config.config_sha256 = Some(actingcommand_runtime_config::sha256_text(&bytes));
    config.config_generation = installation.map(|selected| selected.selection().generation);
    config.source_root = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    // Workflow #360: the vision provider is no installed file any more; a selection that
    // still names one predates the vision root and is refused.
    if installation.is_some_and(|selected| selected.selection().provider.is_some()) {
        return Err(ActingdCode::InstallProviderRetired.code());
    }
    // The install root a running daemon's selection names, or, for a candidate check, the
    // root its slot lock names (the same root).
    config.install_root = match installation {
        Some(selected) => Some(selected.root().to_path_buf()),
        None => actingcommand_contract::process_slot_lock()
            .map_err(|error| error.code())?
            .map(|lock| lock.root().to_path_buf()),
    };
    Ok(config)
}

impl ActingdConfigFile {
    pub(super) fn maintenance_config(self) -> Result<RuntimeHostConfig, Code> {
        if self.schema_version != CONFIG_SCHEMA_VERSION
            || self.state_root.as_os_str().is_empty()
            || !(16..=1024).contains(&self.secret_fingerprint_salt.len())
        {
            return Err(ActingdCode::MaintenanceConfigInvalid.code());
        }
        Ok(RuntimeHostConfig::new(
            self.state_root,
            self.secret_fingerprint_salt.as_bytes(),
        ))
    }

    /// Workflow #378 A6a: [`Self::assemble`] with its refusal as an outcome: the code, with the
    /// values that name what was refused when the refusing check gives them (a runtime-config
    /// key's `config_key` and why, Workflow #385 P-c and #387 D1c). A start and `check-config`
    /// report it.
    pub(super) fn assemble_or_refusal(self) -> Result<RuntimeAssembly, (Code, Outcome)> {
        let mut refused = None;
        self.assemble_noting(&mut refused).map_err(|code| {
            let outcome = match refused {
                Some(outcome) if outcome.is(code) => outcome,
                _ => Outcome::new(code),
            };
            (code, outcome)
        })
    }

    #[cfg(test)]
    pub(super) fn assemble(self) -> Result<RuntimeAssembly, Code> {
        self.assemble_noting(&mut None)
    }

    /// Assembles the configuration; a refusal whose check gives values notes its outcome in
    /// `refused` (Workflow #378 A6a).
    fn assemble_noting(self, refused: &mut Option<Outcome>) -> Result<RuntimeAssembly, Code> {
        if self.governance_capability_retired {
            return Err(ActingdCode::GovernanceCapabilityRetired.code());
        }
        if self.vision_provider_manifest_retired {
            return Err(ActingdCode::VisionProviderManifestRetired.code());
        }
        if self.schema_version != CONFIG_SCHEMA_VERSION
            || self.state_root.as_os_str().is_empty()
            || !(16..=1024).contains(&self.secret_fingerprint_salt.len())
        {
            return Err(ActingdCode::ConfigInvalid.code());
        }
        let governance_policy = self
            .governance
            .as_ref()
            .map(GovernanceConfigFile::policy)
            .transpose()?
            .unwrap_or_default();
        let bind_host = self
            .bind_host
            .parse::<IpAddr>()
            .map_err(|_| ActingdCode::BindHostInvalid.code())?;
        if !bind_host.is_loopback() {
            return Err(ActingdCode::BindHostNotLoopback.code());
        }
        if self
            .mumu_root
            .as_ref()
            .is_some_and(|root| root.as_os_str().is_empty() || !root.is_absolute())
        {
            return Err(ActingdCode::MumuRootInvalid.code());
        }
        // Workflow #318 cfg2: a pressure streak outside 1..=30 keeps its own refusal; a value of
        // another type is the configuration mechanism's (`config_value_invalid`, below).
        for key in [
            monitor::PRESSURE_START_SAMPLES,
            monitor::PRESSURE_END_SAMPLES,
        ] {
            let out_of_range = self
                .performance
                .as_ref()
                .and_then(|section| section.value_at(key))
                .filter(|value| value.is_i64() || value.is_u64())
                .is_some_and(|value| {
                    !value.as_i64().is_some_and(|samples| {
                        (1..=monitor::MAX_PRESSURE_STREAK_SAMPLES).contains(&samples)
                    })
                });
            if out_of_range {
                return Err(ActingdCode::InvalidPressureSamples.code());
            }
        }
        let device_paths = self.device_paths.unwrap_or_default();
        device_paths.validate()?;
        let (env_overrides, ignored_env_overrides) =
            env_overrides(self.allow_env_overrides.unwrap_or(false), |name| {
                std::env::var_os(name)
            });
        // Workflow #337: resolved once; every device instance carries the result.
        let installed_adb = ac_adb::InstalledAdb::detect();
        let mut adb_requirement = ac_adb::AdbRequirement::new(installed_adb.clone());
        let mut instances = self.instances;
        let mut startup_packages = BTreeMap::new();
        let mut resource_packages = BTreeMap::new();
        let mut recognition_packages = BTreeMap::new();
        let mut instance_ids = Vec::with_capacity(instances.len());
        // Workflow #387 D1c: each instance's alias and id, for the stuck-recovery settings the
        // configuration mechanism resolves below.
        let mut instance_aliases = Vec::with_capacity(instances.len());
        // Workflow #387 D1b: the members of the per-instance section as the file names them, the
        // file layer the configuration mechanism resolves (`instances[]` by `instance_id`).
        let mut instance_members = Vec::with_capacity(instances.len());
        // Workflow #398 RD-1b: the handset instances, in configuration order.
        let mut handsets = Vec::new();
        // Workflow #369 S6a: the instances whose emulator the daemon starts, in configuration
        // order.
        let mut emulator_autostart = Vec::new();
        for instance in &mut instances {
            if instance.fixture_backend.is_none()
                && device_binding::check_device(instance)
                    .map_err(|_| ActingdCode::DeviceConfigInvalid.code())?
                    == DeviceKind::Handset
            {
                handsets.push(instance.alias.clone());
            }
            instance.device_paths = device_paths.clone();
            instance.env_overrides = env_overrides.clone();
            if instance.fixture_backend.is_none() {
                instance.adb = ac_adb::InstanceAdb::resolve(
                    installed_adb.as_ref(),
                    instance.adb_path.as_deref(),
                );
                if instance.adb.selected().is_some() {
                    adb_requirement.add_user(&instance.alias);
                }
            }
            // A cool-down the file names is checked here, as before; one it leaves out is the
            // `recovery_ladder` section's (Workflow #387 D1c), resolved below.
            if let Some(cooldown_secs) = instance.stuck_recovery_cooldown_secs {
                actingcommand_contract::InstanceStuckRecovery {
                    enabled: instance.stuck_recovery.unwrap_or_default(),
                    cooldown_secs,
                }
                .validate()
                .map_err(|_| ActingdCode::StuckRecoveryCooldownInvalid.code())?;
            }
            // Workflow #369 S6a: only a discovery-bound instance has an emulator to start.
            if instance.start_emulator == Some(true) {
                if instance.start_emulator_refused()? {
                    return Err(ActingdCode::StartEmulatorRequiresDiscoveryBinding.code());
                }
                emulator_autostart.push(instance.alias.clone());
            }
            let instance_id = manifest::instance_id_text(instance.instance_id)?;
            instance_aliases.push((instance.alias.clone(), instance_id.clone()));
            let mut members = serde_json::Map::new();
            members.insert(
                "instance_id".to_owned(),
                serde_json::Value::String(instance_id.clone()),
            );
            members.insert(
                actingcommand_runtime_config::STUCK_RECOVERY.to_owned(),
                serde_json::json!(instance.stuck_recovery),
            );
            members.insert(
                actingcommand_runtime_config::STUCK_RECOVERY_COOLDOWN_SECS.to_owned(),
                serde_json::json!(instance.stuck_recovery_cooldown_secs),
            );
            members.insert(
                actingcommand_runtime_config::scheduling::SECTION.to_owned(),
                serde_json::to_value(&instance.scheduling)
                    .map_err(|_| ActingdCode::ConfigManifestInvalid.code())?,
            );
            // Workflow #369 S6a: reported as `instance.<id>.start_emulator` when the file names it.
            members.insert(
                actingcommand_runtime_config::START_EMULATOR.to_owned(),
                serde_json::json!(instance.start_emulator),
            );
            // Workflow #398 RD-1b: `device.kind` is reported as `instance.<id>.device_kind`.
            if let Some(device) = &instance.device {
                members.insert(
                    "device".to_owned(),
                    serde_json::to_value(device)
                        .map_err(|_| ActingdCode::ConfigManifestInvalid.code())?,
                );
            }
            // Workflow #384 D2-S2: the location is reported as
            // `instance.<id>.recognition_package`, resolved as it is used. An empty path stays
            // empty, so the binding check reports it unreadable.
            if let Some(recognition) = instance.recognition_package.take() {
                let path = if recognition.package.as_os_str().is_empty()
                    || recognition.package.is_absolute()
                {
                    recognition.package
                } else {
                    self.source_root.join(recognition.package)
                };
                members.insert(
                    actingcommand_runtime_config::RECOGNITION_PACKAGE.to_owned(),
                    serde_json::json!({ "package": path.to_string_lossy() }),
                );
                recognition_packages.insert(instance.alias.clone(), path);
            }
            instance_members.push(serde_json::Value::Object(members));
            instance_ids.push(instance_id);
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
                    return Err(ActingdCode::InstanceConfigInvalid.code());
                }
                startup_packages.insert(
                    instance.alias.clone(),
                    startup_package.request(&self.source_root)?,
                );
            }
        }
        // Workflow #398 RD-1b (model-398 v1.1 §3.2, §3.6): the private binding file, read once
        // and only when a handset is configured.
        let (unbound_handsets, unused_device_bindings) = if handsets.is_empty() {
            (Vec::new(), Vec::new())
        } else {
            let mut bound = device_binding::bind_handsets(&self.state_root, &handsets)
                .map_err(|refused| refused.code())?;
            for instance in &mut instances {
                instance.handset = bound.endpoints.remove(&instance.alias);
            }
            (bound.unbound, bound.unused)
        };
        let configured_aliases = instances
            .iter()
            .map(|instance| instance.alias.clone())
            .collect::<BTreeSet<_>>();
        let instances = instances
            .into_iter()
            .map(InstanceConfig::backend)
            .collect::<Result<Vec<_>, _>>()?;
        let vision = self
            .vision
            .map(|vision| vision.assemble(&self.source_root, self.install_root.as_deref()))
            .transpose()?;
        let provider = ConfiguredProvider::new(instances, self.mumu_root, vision, env_overrides);
        let policy = self
            .policy
            .map(|policy| policy.assemble(&self.source_root))
            .transpose()?;
        if let Some(policy) = policy.as_ref() {
            policy.validate_registry_modes(&provider)?;
            policy.validate_instance_identities(&configured_aliases)?;
        }
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
                .with_frame_retention_dedup(
                    self.frame_retention_dedup_error.unwrap_or(true),
                    self.frame_retention_dedup_lab.unwrap_or(false),
                )
                .with_bind_address(SocketAddr::new(
                    bind_host,
                    self.bind_port.unwrap_or_default(),
                ))
                .with_policy_cadence(policy_cadence.clone())
                .with_governance_policy(governance_policy);
        let instances_startup_package_count = startup_packages.len();
        // Workflow #387 D1c: the sections the configuration mechanism resolves, as the file
        // names them, before the standard-pack root is resolved against the file's directory.
        let file_layer = serde_json::json!({
            "instances": instance_members,
            "recovery_ladder": serde_json::to_value(&self.recovery_ladder)
                .map_err(|_| ActingdCode::ConfigManifestInvalid.code())?,
            "host_claims": serde_json::to_value(&self.host_claims)
                .map_err(|_| ActingdCode::ConfigManifestInvalid.code())?,
            "standard_packs": serde_json::to_value(&self.standard_packs)
                .map_err(|_| ActingdCode::ConfigManifestInvalid.code())?,
            "performance": serde_json::to_value(&self.performance)
                .map_err(|_| ActingdCode::ConfigManifestInvalid.code())?,
            "scheduling": serde_json::to_value(&self.scheduling)
                .map_err(|_| ActingdCode::ConfigManifestInvalid.code())?,
            "ledger_observer": serde_json::to_value(&self.ledger_observer)
                .map_err(|_| ActingdCode::ConfigManifestInvalid.code())?,
            "lab_module_authoring": self.lab_module_authoring,
            "lab_module_debugging": self.lab_module_debugging,
            "device_link": serde_json::to_value(&self.device_link)
                .map_err(|_| ActingdCode::ConfigManifestInvalid.code())?,
            "device_state": serde_json::to_value(&self.device_state)
                .map_err(|_| ActingdCode::ConfigManifestInvalid.code())?,
        });
        let standard_packs_root = self
            .standard_packs
            .map(|section| {
                if section.root.as_os_str().is_empty() {
                    Err(SharedCode::ConfigDecodeFailed.code())
                } else if section.root.is_absolute() {
                    Ok(section.root)
                } else {
                    Ok(self.source_root.join(section.root))
                }
            })
            .transpose()?;
        // Workflow #387 D1b: the defaults file and the rows the configuration mechanism resolves
        // itself (D1c: the `recovery_ladder`, `host_claims` and `standard_packs` sections; P-c:
        // the `performance` section; #386 SR-1: `scheduling`; #389 U-2: the two Lab options;
        // then the per-instance section), from the members the file named.
        let (_, defaults) = actingcommand_runtime_config::DefaultsFile::shipped()
            .map_err(|_| ActingdCode::ConfigManifestInvalid.code())?;
        let mechanism_parameters = actingcommand_runtime_config::render_parameters(
            &actingcommand_runtime_config::mechanism_rows(),
            &instance_ids,
            &defaults,
            &file_layer,
            &|_| None,
        )
        .map_err(|failure| {
            *refused = Some(failure.outcome());
            failure.failure_code().code()
        })?;
        // Each instance's stuck-recovery settings as the file layer resolves them: its own
        // members, or the `recovery_ladder` section's (M387 §5.5 `section:`). The Runtime reads
        // the values in effect; check-config reports these.
        let stuck_recovery = instance_aliases
            .iter()
            .map(|(alias, instance_id)| {
                stuck_recovery_settings(&mechanism_parameters, instance_id)
                    .map(|settings| (alias.clone(), settings))
            })
            .collect::<Result<BTreeMap<_, _>, _>>()?;
        let config_selection =
            self.config_sha256
                .map(|config_sha256| actingcommand_contract::ConfigSelection {
                    config_generation: self.config_generation,
                    config_sha256,
                    defaults_sha256: actingcommand_runtime_config::shipped_defaults_sha256(),
                });
        // Workflow #385 P-c: the monitor and the controller take the section's values in
        // effect.
        let (performance_monitor, performance_control) =
            performance_settings(&mechanism_parameters, refused)?;
        host = host
            .with_performance_monitor(performance_monitor)
            .with_performance_control(performance_control)
            .with_startup_packages(startup_packages)
            .with_stuck_recovery(stuck_recovery)
            .with_standard_packs_root(standard_packs_root);
        // Every effective value is read back from `host`; the file only says what it named.
        let manifest = manifest::build(&manifest::ManifestInputs {
            host: &host,
            bind_port_explicit: self.bind_port.is_some(),
            device_diagnostic_mode_explicit: self.device_diagnostic_mode.is_some(),
            frame_retention_enabled: self.frame_retention_enabled,
            dedup_error_explicit: self.frame_retention_dedup_error.is_some(),
            dedup_lab_explicit: self.frame_retention_dedup_lab.is_some(),
            capacity_thresholds_explicit: self.capacity_thresholds.is_some(),
            secret_fingerprint_salt_bytes: self.secret_fingerprint_salt.len(),
            mumu_root: provider.mumu_root(),
            device_paths: device_paths.entries(),
            allow_env_overrides: self.allow_env_overrides,
            ignored_env_overrides: &ignored_env_overrides,
            governance_allowed_clients_explicit: self.governance.is_some(),
            agent_dispatcher: agent_dispatcher_budget,
            policy_configured: policy.is_some(),
            catalog_transition: policy
                .as_ref()
                .and_then(|policy| policy.catalog_transition.as_ref()),
            policy_monitor: policy.as_ref().map(|policy| policy.monitor),
            vision_provider_configured: provider.vision.is_some(),
            instances_count: provider.instance_count(),
            instances_deferred_count: provider.deferred.len(),
            instances_startup_package_count,
            mechanism_parameters,
        })?;
        // Workflow #387 D1b: the whole-section checks of the file layer, with the facts of other
        // crates they read (§5.3); a start and check-config refuse the same way.
        actingcommand_runtime_config::EffectiveConfig::from_manifest(&manifest)
            .and_then(|effective| {
                effective.check(&actingcommand_runtime_config::CheckContext::new(
                    host.scheduler().maximum_client_heartbeat_interval_ms,
                    actingcommand_runtime_host::recovery_ladder_device_bounds(),
                ))
            })
            .map_err(|failure| {
                *refused = Some(failure.outcome());
                failure.failure_code().code()
            })?;
        host = host.with_config_manifest(manifest.clone());
        if let Some(selection) = &config_selection {
            host = host.with_config_selection(selection.clone());
        }
        if let Some(dispatcher) = self.agent_dispatcher {
            host = host.with_agent_dispatcher(dispatcher.runtime_config()?);
        }
        let policy = if let Some(policy) = policy {
            // Workflow #369 E3 (#670 ruling 1): a restart closes a scheduled dispatch whose run
            // was cut before its first task event; the host tells them apart by these refs.
            host = host
                .with_policy_inputs(policy.inputs)
                .with_procedure_manifest(policy.procedure_manifest)
                .with_scheduled_procedures(policy.scheduled_tasks.keys().cloned());
            Some(PolicyBootstrap {
                catalog_approval_ids: policy.catalog_approval_ids,
                catalog: policy.catalog,
                catalog_transition: policy.catalog_transition,
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
            config_selection,
            resource_packages,
            recognition_packages,
            ignored_env_overrides,
            adb_requirement,
            unbound_handsets,
            unused_device_bindings,
            emulator_autostart,
        })
    }

    /// Workflow #398 RD-1b: the device refusal `assemble` meets first, if any, with its detail:
    /// the same checks in the same order (each device instance's `device`, then the private
    /// binding file when a handset is configured). `check-config` and the start attach it when
    /// `assemble` refuses with its code.
    pub(super) fn device_refusal(&self) -> Option<DeviceRefusal> {
        let mut handsets = Vec::new();
        for instance in self
            .instances
            .iter()
            .filter(|instance| instance.fixture_backend.is_none())
        {
            match device_binding::check_device(instance) {
                Ok(DeviceKind::Handset) => handsets.push(instance.alias.clone()),
                Ok(DeviceKind::Emulator) => {}
                Err(refused) => return Some(DeviceRefusal::Config(refused)),
            }
        }
        if handsets.is_empty() {
            return None;
        }
        device_binding::bind_handsets(&self.state_root, &handsets)
            .err()
            .map(DeviceRefusal::BindingFile)
    }

    /// Workflow #369 S6a: the alias of the first instance whose `start_emulator: true`
    /// `assemble` refuses with `start_emulator_requires_discovery_binding`, the refusal's
    /// `alias`. `check-config` attaches it when `assemble` refuses with that code.
    pub(super) fn start_emulator_refusal(&self) -> Option<String> {
        self.instances
            .iter()
            .find(|instance| {
                instance.start_emulator == Some(true)
                    && matches!(instance.start_emulator_refused(), Ok(true))
            })
            .map(|instance| instance.alias.clone())
    }
}

/// The warnings the set but ignored `ACTINGCOMMAND_*` variables produce (Workflow #318 cfg3):
/// one `env_override_ignored` with the variable's name as `variable` per variable (Workflow #378
/// D6).
pub(super) fn env_override_warnings(ignored: &[&str]) -> Vec<Outcome> {
    ignored
        .iter()
        .map(|variable| Outcome::new(ActingdCode::EnvOverrideIgnored).variable(*variable))
        .collect()
}

/// Workflow #318 (cfg3): the environment fallbacks `assemble` injects. Allowed, every
/// `EnvOverrides::VARIABLES` entry is read through `lookup`; otherwise nothing is injected
/// and every variable that is set is returned as ignored, so it is reported, not dropped.
pub(super) fn env_overrides(
    allowed: bool,
    lookup: impl Fn(&str) -> Option<OsString>,
) -> (EnvOverrides, Vec<&'static str>) {
    if allowed {
        (EnvOverrides::from_lookup(lookup), Vec::new())
    } else {
        let ignored = EnvOverrides::VARIABLES
            .into_iter()
            .filter(|name| lookup(name).is_some())
            .collect();
        (EnvOverrides::default(), ignored)
    }
}

struct PolicyAssembly {
    inputs: PolicyInputSnapshot,
    procedure_manifest: ProcedureManifest,
    catalog_approval_ids: Vec<String>,
    catalog: CatalogSources,
    catalog_transition: Option<CatalogTransitionRequest>,
    scheduled_tasks: BTreeMap<String, ScheduledProcedureTask>,
    scheduled_instance_scopes: Vec<(String, String)>,
    /// Workflow #397: `policy.monitor`, resolved.
    monitor: manifest::PolicyMonitorParameters,
}

impl PolicyConfigFile {
    fn assemble(self, source_root: &Path) -> Result<PolicyAssembly, Code> {
        if self.procedure_manifest.is_empty() || self.procedure_manifest.len() > MAX_TASKS {
            return Err(ActingdCode::ProcedureManifestSizeInvalid.code());
        }
        let mut bindings = Vec::with_capacity(self.procedure_manifest.len());
        let mut scheduled_tasks = BTreeMap::new();
        for configured in self.procedure_manifest {
            let (binding, scheduled_task) = procedure_binding(configured, source_root)?;
            if let Some((procedure_ref, request)) = scheduled_task
                && scheduled_tasks.insert(procedure_ref, request).is_some()
            {
                return Err(ActingdCode::ProcedureTaskDuplicate.code());
            }
            bindings.push(binding);
        }
        let procedure_manifest = ProcedureManifest::new(bindings)
            .map_err(|_| ActingdCode::ProcedureManifestInvalid.code())?;
        let catalog = self.catalog.sources(source_root)?;
        let compiled =
            compile_catalog(&catalog).map_err(|_| SharedCode::PolicyCatalogCompileFailed.code())?;
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
            return Err(ActingdCode::PolicyCatalogApprovalMismatch.code());
        }
        // Workflow #361 C1: every configured id must be one the policy driver can record: the
        // approval decision record's own rule (`approval:` prefix, `[a-z0-9._:-]`), checked
        // here so a refused id stops before the catalog is activated.
        for approval_id in &self.catalog_approval_ids {
            actingcommand_contract::ApprovalReasonCode::new("configured_catalog_approval")
                .and_then(|reason| {
                    ApprovalDecisionRecord::new(
                        approval_id.clone(),
                        ApprovalDisposition::Approved,
                        ApprovalTarget::Catalog {
                            catalog_hash: compiled.catalog_hash().to_owned(),
                            catalog_version: compiled.summary().catalog_version,
                        },
                        reason,
                    )
                })
                .map_err(|_| SharedCode::PolicyCatalogApprovalInvalid.code())?;
        }
        // Workflow #361 C1: every catalog task's procedure is bound by the manifest; evaluation
        // binding would otherwise refuse its first intent after activation.
        if compiled
            .catalog()
            .tasks
            .tasks
            .iter()
            .any(|task| procedure_manifest.binding(&task.procedure_ref).is_none())
        {
            return Err(SharedCode::ProcedureManifestEntryMissing.code());
        }
        // Workflow #361 A, C1: the transition's shape; whether it applies is decided against
        // the ledger at startup (and previewed by `check-config`).
        let catalog_transition = self
            .catalog_transition
            .as_ref()
            .map(CatalogTransitionConfigFile::request)
            .transpose()?;
        let monitor = self.monitor.unwrap_or_default().parameters()?;
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
            catalog_transition,
            scheduled_tasks,
            scheduled_instance_scopes,
            monitor,
        })
    }
}

impl PolicyAssembly {
    /// Workflow #361 C1: the policy input authority check of the first cycle
    /// (`policy_instance_metadata_untrusted`, `policy_resource_metadata_untrusted`), made at
    /// assembly: the policy instance set equals the configured alias set, and every policy
    /// instance's host is in `resources.hosts`.
    fn validate_instance_identities(
        &self,
        configured_aliases: &BTreeSet<String>,
    ) -> Result<(), Code> {
        let instances = &self.inputs.facts().instances;
        let policy_aliases = instances
            .iter()
            .map(|instance| instance.instance_id.clone())
            .collect::<BTreeSet<_>>();
        if policy_aliases != *configured_aliases {
            return Err(ActingdCode::PolicyInstanceSetMismatch.code());
        }
        let hosts = self
            .inputs
            .resources()
            .hosts
            .iter()
            .map(|host| host.host_id.as_str())
            .collect::<BTreeSet<_>>();
        if instances
            .iter()
            .any(|instance| !hosts.contains(instance.host_id.as_str()))
        {
            return Err(ActingdCode::PolicyInstanceHostUnknown.code());
        }
        Ok(())
    }

    fn validate_registry_modes(&self, provider: &ConfiguredProvider) -> Result<(), Code> {
        for (procedure_ref, instance_alias) in &self.scheduled_instance_scopes {
            let scheduled = self
                .scheduled_tasks
                .get(procedure_ref)
                .ok_or(ActingdCode::ScheduledExecutionBindingMissing.code())?;
            let actual = provider
                .mode_for_alias(instance_alias)
                .ok_or(ActingdCode::ScheduledExecutionInstanceUnknown.code())?;
            if actual != scheduled.mode {
                return Err(ActingdCode::ScheduledExecutionBackendModeMismatch.code());
            }
        }
        let modes = provider.modes();
        for scheduled in self.scheduled_tasks.values() {
            if !modes.values().any(|mode| mode == &scheduled.mode) {
                return Err(ActingdCode::ScheduledExecutionBackendModeUnavailable.code());
            }
        }
        Ok(())
    }
}

impl PolicyCatalogConfigFile {
    fn sources(self, source_root: &Path) -> Result<CatalogSources, Code> {
        let sources = CatalogSources {
            tasks: read_catalog_document(source_root, &self.tasks)?,
            pools: read_catalog_document(source_root, &self.pools)?,
            activity: read_catalog_document(source_root, &self.activity)?,
            timeline: read_catalog_document(source_root, &self.timeline)?,
            selection: None,
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
        .ok_or(ActingdCode::PolicyCatalogSizeInvalid.code())?;
        if total_bytes > MAX_CATALOG_BYTES {
            return Err(ActingdCode::PolicyCatalogSizeInvalid.code());
        }
        Ok(sources)
    }
}

fn procedure_binding(
    configured: ProcedureBindingConfigFile,
    source_root: &Path,
) -> Result<AssembledProcedureBinding, Code> {
    let ProcedureBindingConfigFile {
        procedure_ref,
        operation_id,
        yield_points,
        scheduled_execution,
    } = configured;
    if yield_points.len() > MAX_REFERENCES_PER_TASK {
        return Err(ActingdCode::ProcedureBindingSizeInvalid.code());
    }
    // Workflow #336 PKG-S3/PKG-S4: the binding names no package content (tuple v3, M-PKG D13)
    // and the configuration names its package by its path only; the binding carries the
    // scheduled request's path as its location.
    let binding = ProcedureBinding::new(procedure_ref.clone(), operation_id, yield_points)
        .map_err(|_| ActingdCode::ProcedureBindingInvalid.code())?;
    let scheduled_task = match scheduled_execution {
        None => None,
        Some(ScheduledExecutionConfigFile::FixtureSimulation { package_path }) => Some((
            procedure_ref,
            ScheduledProcedureTask {
                request: contained_task_request(source_root, package_path)?,
                mode: ScheduledExecutionMode::FixtureSimulation,
            },
        )),
        Some(ScheduledExecutionConfigFile::DeviceRegistry { package_path }) => Some((
            procedure_ref,
            ScheduledProcedureTask {
                request: contained_task_request(source_root, package_path)?,
                mode: ScheduledExecutionMode::DeviceRegistry,
            },
        )),
    };
    let binding = match &scheduled_task {
        Some((_, scheduled)) => binding.with_package_path(scheduled.request.package_path()),
        None => binding,
    };
    Ok((binding, scheduled_task))
}

/// The request of a procedure binding (Workflow #336 PKG-S4, option 丙): its package by its
/// path only. A relative path resolves against the configuration
/// file's directory, and the path keeps its components so containment can refuse a link. The
/// path chooses how the package is read, as containment does (M-PKG §3.2): a directory is a
/// content directory, a `.zip` or `.json` file (ASCII case-insensitive) a container, an
/// assembled ZIP among them; another file is `procedure_package_container_unsupported`. Nothing
/// is opened or hashed here.
fn contained_task_request(
    source_root: &Path,
    package_path: Option<PathBuf>,
) -> Result<ContainedTaskRequest, Code> {
    let package_path = package_path.ok_or(ActingdCode::ProcedurePackagePathMissing.code())?;
    let path = if package_path.is_absolute() {
        package_path
    } else {
        source_root.join(package_path)
    };
    let path = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .map_err(|_| ActingdCode::ProcedurePackageUnavailable.code())?
            .join(path)
    };
    let metadata =
        fs::metadata(&path).map_err(|_| ActingdCode::ProcedurePackageUnavailable.code())?;
    let container = || {
        path.extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                extension.eq_ignore_ascii_case("zip") || extension.eq_ignore_ascii_case("json")
            })
    };
    if metadata.is_file() && !container() {
        return Err(ActingdCode::ProcedurePackageContainerUnsupported.code());
    }
    if !metadata.is_file() && !metadata.is_dir() {
        return Err(ActingdCode::ProcedurePackageNotRegular.code());
    }
    ContainedTaskRequest::new(path.to_string_lossy().into_owned())
        .and_then(|request| {
            request.with_response_deadline_ms(ContainedTaskRequest::MAX_RESPONSE_DEADLINE_MS)
        })
        .map_err(|_| ActingdCode::ProcedureTaskRequestInvalid.code())
}

fn read_catalog_document(
    source_root: &Path,
    configured_path: &Path,
) -> Result<CatalogDocumentSource, Code> {
    if configured_path.as_os_str().is_empty() {
        return Err(ActingdCode::PolicyCatalogPathInvalid.code());
    }
    let path = if configured_path.is_absolute() {
        configured_path.to_path_buf()
    } else {
        source_root.join(configured_path)
    };
    let metadata = fs::metadata(&path).map_err(|_| SharedCode::PolicyCatalogUnavailable.code())?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_DOCUMENT_BYTES as u64 {
        return Err(ActingdCode::PolicyCatalogDocumentSizeInvalid.code());
    }
    let bytes = fs::read(&path).map_err(|_| ActingdCode::PolicyCatalogReadFailed.code())?;
    Ok(CatalogDocumentSource::new(
        format!("file://{}", path.to_string_lossy().replace('\\', "/")),
        bytes,
    ))
}

impl AgentDispatcherConfigFile {
    fn runtime_config(self) -> Result<AgentDispatcherConfig, Code> {
        AgentDispatcherConfig::new(
            self.max_attempts,
            self.max_session_ms,
            self.max_projection_events,
        )
        .map_err(|_| SharedCode::AgentDispatcherConfigInvalid.code())
    }
}

impl InstanceConfig {
    fn backend(self) -> Result<ConfiguredInstance, Code> {
        if self.fixture_backend.is_some() {
            self.fixture_backend().map(ConfiguredInstance::Spec)
        } else if self.handset.is_some() {
            // Workflow #398 RD-1b: a handset is addressed by its serial; it has no host or port.
            let adb_path = self.configured_adb_path()?;
            self.device_registration(adb_path, default_device_host(), None)
                .map(InstanceSpec::real)
                .map(ConfiguredInstance::Spec)
        } else if let Some(key) = self.binding_key()? {
            self.deferred_backend(key)
        } else {
            self.device_backend().map(ConfiguredInstance::Spec)
        }
    }

    fn binding_key(&self) -> Result<Option<InstanceBindingKey>, Code> {
        match (self.instance_index, self.instance_name.as_deref()) {
            (None, None) => Ok(None),
            (Some(_), Some(_)) => Err(ActingdCode::InstanceBindingKeyInvalid.code()),
            (Some(index), None) => Ok(Some(InstanceBindingKey::Index(index))),
            (None, Some(name)) => {
                if name.trim().is_empty()
                    || name.len() > actingcommand_device::MAX_MUMU_INSTANCE_NAME_BYTES
                    || name.chars().any(char::is_control)
                {
                    return Err(ActingdCode::InstanceBindingKeyInvalid.code());
                }
                Ok(Some(InstanceBindingKey::Name(name.to_owned())))
            }
        }
    }

    /// Workflow #369 S6a: whether `start_emulator: true` is refused on this instance, which has
    /// no emulator the daemon can start: a fixture, or an instance bound otherwise than by
    /// discovery (`start_emulator_requires_discovery_binding`).
    fn start_emulator_refused(&self) -> Result<bool, Code> {
        Ok(self.fixture_backend.is_some() || self.binding_key()?.is_none())
    }

    /// Validates everything that does not need discovery; the ADB target is completed later.
    /// Declared `adb_path`/`host`/`port` stay declared values to be cross-checked; no default
    /// host or port applies to a discovery-bound instance. An `adb_path` that is absent or
    /// names the install root's adb selects that adb instead (Workflow #337).
    fn deferred_backend(self, key: InstanceBindingKey) -> Result<ConfiguredInstance, Code> {
        if self.serial.is_some() {
            return Err(ActingdCode::InstanceBindingKeyInvalid.code());
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
            return Err(ActingdCode::InstanceConfigInvalid.code());
        }
        self.application_id
            .as_ref()
            .filter(|value| !value.trim().is_empty())
            .ok_or(ActingdCode::ApplicationIdentityMissing.code())?;
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

    fn backend_choices(&self) -> Result<(TouchBackendChoice, CaptureBackendChoice), Code> {
        let touch_backend = self
            .touch_backend
            .as_deref()
            .ok_or(ActingdCode::TouchBackendInvalid.code())?;
        let capture_backend = self
            .capture_backend
            .as_deref()
            .ok_or(ActingdCode::CaptureBackendInvalid.code())?;
        let requested = TouchBackendChoice::parse(touch_backend)
            .map_err(|_| ActingdCode::TouchBackendInvalid.code())?;
        if matches!(
            requested,
            TouchBackendChoice::Auto | TouchBackendChoice::AutoFastest
        ) {
            return Err(ActingdCode::TouchBackendMustBeExplicit.code());
        }
        let capture_requested = CaptureBackendChoice::parse(capture_backend)
            .map_err(|_| ActingdCode::CaptureBackendInvalid.code())?;
        if matches!(
            capture_requested,
            CaptureBackendChoice::Auto | CaptureBackendChoice::AutoFastest
        ) {
            return Err(ActingdCode::CaptureBackendMustBeExplicit.code());
        }
        // #398 R1 §5.1 (RD-3i) and §3.1 (RD-1b): which backends the instance's device kind
        // has is checked with its `device` member (`device_binding::check_device`).
        Ok((requested, capture_requested))
    }

    /// Workflow #337 (A4): an absent `adb_path` takes the install root's adb, checked by
    /// `ac_adb::require`; outside an install root it is still refused.
    fn configured_adb_path(&self) -> Result<String, Code> {
        self.adb_path
            .clone()
            .or_else(|| self.adb.selected().map(ac_adb::InstalledAdb::path_string))
            .ok_or(ActingdCode::InstanceConfigInvalid.code())
    }

    fn device_backend(self) -> Result<InstanceSpec, Code> {
        let adb_path = self.configured_adb_path()?;
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
    ) -> Result<ExecutionBackendRegistration, Code> {
        let connect = self.connect.unwrap_or_else(enabled);
        if adb_path.trim().is_empty()
            || host.trim().is_empty()
            || port == Some(0)
            || self
                .serial
                .as_ref()
                .is_some_and(|value| value.trim().is_empty())
        {
            return Err(ActingdCode::InstanceConfigInvalid.code());
        }
        let (requested, capture_requested) = self.backend_choices()?;
        let nemu_app_index = match (requested, self.nemu_app_index) {
            (TouchBackendChoice::NemuIpc, Some(index))
                if capture_requested == CaptureBackendChoice::NemuIpc =>
            {
                Some(
                    actingcommand_device::NemuAppIndex::try_from(index)
                        .map_err(|_| ActingdCode::NemuAppIndexInvalid.code())?,
                )
            }
            (TouchBackendChoice::NemuIpc, _) => {
                return Err(ActingdCode::NemuPairedInputConfigurationMissing.code());
            }
            (_, Some(_)) => return Err(SharedCode::NemuAppIndexRequiresPairedInput.code()),
            (_, None) => None,
        };
        let application_id = self
            .application_id
            .filter(|value| !value.trim().is_empty())
            .ok_or(ActingdCode::ApplicationIdentityMissing.code())?;
        let mut adb = AdbConfig {
            adb_path,
            ..AdbConfig::default()
        };
        if let Some(timeout) = bounded_duration(self.command_timeout_ms)? {
            adb.command_timeout = timeout;
        }
        // Workflow #398 RD-1a: a configured serial is shown only as `handset:<alias>`. RD-1b
        // (model-398 v1.1 §3.1): a handset is addressed by its bound serial (an unbound one by
        // the stand-in no session opens on) and is never `adb connect`ed.
        let handset = self.handset.clone();
        let target = match &handset {
            Some(handset) => DeviceTarget {
                serial: Some(handset.binding().map_or_else(
                    || DeviceSerial::unbound_handset(&self.alias),
                    |binding| binding.serial().clone(),
                )),
                host,
                port: port.unwrap_or(0),
                connect: false,
            },
            None => DeviceTarget {
                serial: self
                    .serial
                    .map(|serial| DeviceSerial::handset(serial, &self.alias)),
                host,
                port: port.unwrap_or(0),
                connect,
            },
        };
        let mut maatouch = MaaTouchConfig::default();
        // The injected `ACTINGCOMMAND_MINITOUCH_PATH` (only under `allow_env_overrides`)
        // replaces the bundled path; every configured path below keeps precedence over it.
        let mut minitouch = MinitouchConfig::from_env_overrides(&self.env_overrides);
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
        // The injected environment fallbacks (empty unless `allow_env_overrides`) apply only
        // where a daemon-level path below is absent; otherwise discovery applies.
        let mut capture = CaptureBackendConfig::new(adb.clone(), target.clone())
            .with_requested(capture_requested)
            .with_env_overrides(&self.env_overrides);
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
        .map(|registration| match handset {
            Some(handset) => registration.with_handset(handset),
            None => registration,
        })
        .map_err(|_| ActingdCode::InstanceRegistrationInvalid.code())
    }

    fn fixture_backend(self) -> Result<InstanceSpec, Code> {
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
            || self.device.is_some()
        {
            return Err(ActingdCode::FixtureDeviceFieldsForbidden.code());
        }
        let configured = self
            .fixture_backend
            .ok_or(ActingdCode::FixtureBackendMissing.code())?;
        if actingcommand_contract::validate_instance_alias(&self.alias).is_err()
            || configured.frames.is_empty()
            || configured.frames.len() > MAX_FIXTURE_FRAMES
            || configured.max_inputs > MAX_FIXTURE_INPUTS
        {
            return Err(ActingdCode::FixtureBackendInvalid.code());
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
                    .ok_or(ActingdCode::FixtureFrameSizeInvalid.code())?;
                if expected_bytes == 0
                    || expected_bytes > MAX_FIXTURE_FRAME_BYTES
                    || frame.rgb.len() != expected_bytes
                {
                    return Err(ActingdCode::FixtureFrameSizeInvalid.code());
                }
                resident_bytes = resident_bytes
                    .checked_add(expected_bytes)
                    .ok_or(ActingdCode::FixtureResidentSizeInvalid.code())?;
                if resident_bytes > MAX_FIXTURE_RESIDENT_BYTES {
                    return Err(ActingdCode::FixtureResidentSizeInvalid.code());
                }
                Frame::from_pixels(
                    frame.width,
                    frame.height,
                    frame.rgb,
                    PixelFormat::Rgb8,
                    CaptureBackendName::FixtureSimulation,
                )
                .map_err(|_| ActingdCode::FixtureFrameInvalid.code())
            })
            .collect::<Result<Vec<_>, _>>()?;
        InstanceSpec::fixture(
            self.alias,
            self.instance_id,
            FixtureInstanceSpec::new(frames, configured.max_inputs),
        )
        .map_err(|_| ActingdCode::FixtureBackendInvalid.code())
    }
}

impl ConfiguredProvider {
    fn new(
        instances: Vec<ConfiguredInstance>,
        mumu_root: Option<PathBuf>,
        vision: Option<ConfiguredVision>,
        env_overrides: EnvOverrides,
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
            vision,
            mumu_root,
            env_overrides,
        }
    }

    /// The `vision` section, when configured (`check-config` lists it as startup does).
    pub(super) fn vision(&self) -> Option<&ConfiguredVision> {
        self.vision.as_ref()
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

    /// The injected `ACTINGCOMMAND_NEMU_FOLDER` value (`None` unless `allow_env_overrides`),
    /// the resolver's environment rung (`check-config` reporting, discovery).
    pub(super) fn env_nemu_folder(&self) -> Option<&Path> {
        self.env_overrides.nemu_folder.as_deref()
    }

    /// The discovery surface startup and `check-config` register: the configured
    /// `mumu_root` plus the injected environment rung.
    fn discovery_spec(&self) -> DiscoverySpec {
        DiscoverySpec::new(self.mumu_root.clone())
            .with_env_nemu_folder(self.env_overrides.nemu_folder.clone())
    }

    fn instance_count(&self) -> usize {
        self.instances.len() + self.deferred.len()
    }

    /// The registry as configured, before provider startup: no vision provider, and every
    /// discovery-bound instance registered with its stand-in target. `check-config`
    /// validates and reports through it; the daemon registers once, at startup, after
    /// discovery (`assemble_provider`).
    pub(super) fn into_registry(self) -> Result<ExecutionBackendRegistry, RuntimeHostError> {
        let discovery = self.discovery_spec();
        let Self {
            mut instances,
            deferred,
            ..
        } = self;
        instances.extend(deferred.into_iter().map(|entry| entry.stand_in));
        ExecutionBackendRegistry::from_assembly(ProviderAssembly {
            instances,
            vision: None,
            discovery: Some(discovery),
        })
    }
}

fn spec_mode(spec: &InstanceSpec) -> ScheduledExecutionMode {
    match spec.mode() {
        InstanceMode::Real(_) => ScheduledExecutionMode::DeviceRegistry,
        InstanceMode::Fixture(_) => ScheduledExecutionMode::FixtureSimulation,
    }
}

fn bounded_duration(value: Option<u64>) -> Result<Option<Duration>, Code> {
    match value {
        Some(value) if value == 0 || value > MAX_TIMEOUT_MS => {
            Err(ActingdCode::TimeoutInvalid.code())
        }
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
        let issuer = actingcommand_contract::IdentifierIssuer::new().expect("ids");
        let witness = actingcommand_contract::issue_fenced_write(
            actingcommand_contract::LeaseToken::new(
                *issuer.mint_owner_epoch().expect("epoch").transport(),
                *issuer.mint_lease_id().expect("lease").transport(),
                *issuer.mint_instance_id().expect("instance").transport(),
                *issuer.mint_holder_id().expect("holder").transport(),
                100,
            )
            .expect("test step token"),
            1,
            std::num::NonZeroU64::new(1).expect("step"),
            actingcommand_contract::FencedWritePurpose::Business,
        );
        backend
            .tap(&witness, 10, 20)
            .expect("fixture input within budget");
        let error = backend
            .tap(&witness, 10, 20)
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

        // Workflow #398 RD-1a: a configured serial is bound for the whole test process and
        // redacted wherever it appears, so the serial rows are neutral fake serials. A target
        // configured by host and port is labelled `HOST:PORT` and kept as it is, so the address
        // forms (IPv4, IPv6, host name) are rows configured that way (train 3 review fix).
        for (capture, by_serial, endpoint) in [
            (false, true, "TESTSERIAL0006"),
            (true, true, "TESTSERIAL0007"),
            (true, true, "TESTSERIAL0008"),
            (true, true, "TESTSERIAL0009"),
            (false, false, "127.0.0.1:16384"),
            (true, false, "198.51.100.42:16416"),
            (true, false, "[2001:db8::42]:16416"),
            (true, false, "device.example.test:16416"),
        ] {
            let root = TempDir::new().expect("tempdir");
            let id = IdentifierIssuer::new()
                .expect("issuer")
                .mint_instance_id()
                .expect("instance id");
            let missing_adb = root.path().join("missing-adb.exe");
            let mut instance = json!({
                "alias": "neutral.device", "instance_id": id.transport(),
                "application_id": "neutral.application", "adb_path": missing_adb,
                "connect": false,
                "touch_backend": "adb_shell_input", "capture_backend": "adb"
            });
            if by_serial {
                instance["serial"] = json!(endpoint);
            } else {
                let (host, port) = endpoint.rsplit_once(':').expect("HOST:PORT");
                instance["host"] = json!(host);
                instance["port"] = json!(port.parse::<u16>().expect("port"));
            }
            let value = json!({
                "schema_version": CONFIG_SCHEMA_VERSION,
                "state_root": root.path(),
                "bind_host": "127.0.0.1", "bind_port": 0,
                "secret_fingerprint_salt": "0123456789abcdef",
                "instances": [instance]
            });
            let config = serde_json::from_value::<ActingdConfigFile>(value).expect("typed config");
            let assembly = config.assemble().expect("runtime assembly");
            let registry = assembly
                .provider
                .into_registry()
                .expect("configured registry");
            let host = RuntimeHost::start(assembly.host, Arc::new(registry)).expect("runtime host");
            let client = RuntimeClient::connect(
                RuntimeClientConfig::new(
                    root.path(),
                    actingcommand_runtime_client::RuntimeChannel::Cli,
                    EventActor::Cli,
                    EventSource::Cli,
                )
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
            // Ledger revision 3: the native cause is a link of the failure's outcome that
            // carries the device's text as `raw_text`.
            fn foreign_text(
                outcome: &actingcommand_contract::outcome::Outcome,
            ) -> Option<&actingcommand_contract::outcome::Outcome> {
                if outcome.value("raw_text").is_some() {
                    return Some(outcome);
                }
                outcome
                    .causes()
                    .iter()
                    .find_map(|link| foreign_text(link.outcome()))
            }
            let details = events
                .iter()
                .filter_map(|event| {
                    let EventPayload::Runtime(RuntimePayload::Failed(payload)) = event.payload()
                    else {
                        return None;
                    };
                    let lifecycle = payload.lifecycle_failure()?;
                    foreign_text(payload.outcome())
                        .map(|native| (event, payload, lifecycle, native))
                })
                .collect::<Vec<_>>();
            assert_eq!(details.len(), 1, "one private native cause per operation");
            let (event, payload, lifecycle, native) = details[0];
            assert_eq!(lifecycle.entered_event_id(), Some(*failure.event_id()));
            assert_eq!(
                event.links().correlation_id(),
                failure.links().correlation_id()
            );
            assert_eq!(event.sensitivity(), Sensitivity::Sensitive);
            let detail = lifecycle.primary_detail().expect("actual device context");
            assert_eq!(detail.category(), "native");
            assert_eq!(detail.stage(), "adb_ensure_device_get_state");
            assert_eq!(
                detail.backend(),
                if capture {
                    "adb_screencap"
                } else {
                    "adb_shell_input"
                }
            );
            assert_eq!(detail.operation(), "ensure_device");
            // Ledger revision 3: the device's text is foreign text, declared sensitive.
            assert_eq!(
                payload.outcome().foreign_text_sensitivity(),
                Some(Sensitivity::Sensitive)
            );
            assert!(native.value("raw_text_truncated").is_none());
            let text = native
                .value("raw_text")
                .and_then(serde_json::Value::as_str)
                .expect("native cause");
            assert!(text.contains("failed to spawn adb"));
            // Workflow #398 RD-1a (model-398 v1.1 §3.5 rules 2 and 4): a configured serial
            // reaches the native cause only as its label token; a `HOST:PORT` target keeps its
            // address there, as before.
            if by_serial {
                assert!(!text.contains(endpoint));
                assert!(text.contains("[device:handset:neutral.device]"));
            } else {
                assert!(text.contains(endpoint));
            }
            if !capture {
                assert!(text.contains("child_operation=ensure_device"));
            }
            let public = serde_json::to_string(&event.payload().public_projection())
                .expect("public payload");
            assert!(!public.contains(endpoint));
            assert!(!public.contains("failed to spawn adb"));
            assert!(!public.contains("missing-adb.exe"));
        }
    }

    // Workflow #389 U-2 (unified interface model v2.2.3 §4.1, §4.2, §8.2 TU-7; register U4): the
    // two Lab options are read once at start and reported as `config.parameters.top_level`
    // rows: absent means `false` (the Lab is not installed by default); a file that names one
    // reports it explicit. No instruction may change them, and nothing is refused by them in
    // v0.12.0.
    #[test]
    fn u2_the_two_lab_options_are_read_and_reported_and_never_settable() {
        let id = IdentifierIssuer::new()
            .expect("issuer")
            .mint_instance_id()
            .expect("instance id");
        let base = json!({
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
        let default = ConfigParameterSource::Default;
        let explicit = ConfigParameterSource::Explicit;
        for (named, authoring, debugging) in [
            (None, (false, default), (false, default)),
            (Some(true), (true, explicit), (false, default)),
        ] {
            let mut value = base.clone();
            if let Some(authoring) = named {
                value["lab_module_authoring"] = json!(authoring);
            }
            let config = serde_json::from_value::<ActingdConfigFile>(value).expect("typed config");
            let assembly = config.assemble().expect("runtime assembly");
            for (key, (expected, source)) in [
                ("lab_module_authoring", authoring),
                ("lab_module_debugging", debugging),
            ] {
                let parameter = assembly
                    .manifest
                    .parameters
                    .iter()
                    .find(|parameter| parameter.key == key)
                    .unwrap_or_else(|| panic!("{key} is reported"));
                assert_eq!(parameter.value, FactScalar::Boolean(expected), "{key}");
                assert_eq!(parameter.source, source, "{key}");
                assert!(!parameter.settable, "{key}");
                assert_eq!(
                    actingcommand_contract::config_parameter_section(key),
                    actingcommand_contract::CONFIG_TOP_LEVEL_SECTION
                );
            }
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
            (
                None,
                true,
                ConfigParameterSource::Default,
                ActingdCode::ConfigSubsystemFlagAbsent,
            ),
            (
                Some(false),
                false,
                ConfigParameterSource::Explicit,
                ActingdCode::ConfigSubsystemConfiguredOff,
            ),
        ] {
            let mut value = value.clone();
            if let Some(configured) = configured {
                value["frame_retention_enabled"] = json!(configured);
                // Workflow #375 R5c: both dedup switches, each against its default.
                value["frame_retention_dedup_error"] = json!(false);
                value["frame_retention_dedup_lab"] = json!(true);
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
            // Workflow #385 P-c: the streaks are rows of the `performance` section, at the file
            // members they had.
            for (key, expected, effective) in [
                (
                    monitor::PRESSURE_START_SAMPLES,
                    if configured.is_some() { 5 } else { 3 },
                    performance_monitor.pressure_start_samples(),
                ),
                (
                    monitor::PRESSURE_END_SAMPLES,
                    if configured.is_some() { 7 } else { 3 },
                    performance_monitor.pressure_end_samples(),
                ),
            ] {
                let parameter = parameter(key).expect("pressure streak parameter");
                assert_eq!(parameter.value, FactScalar::Integer(expected));
                assert_eq!(parameter.value, FactScalar::Integer(i64::from(effective)));
                assert_eq!(parameter.source, source);
            }
            let sample_interval =
                parameter(monitor::SAMPLE_INTERVAL_MS).expect("sample interval parameter");
            assert_eq!(
                sample_interval.value,
                FactScalar::DurationMs(
                    u64::try_from(performance_monitor.sample_interval().as_millis())
                        .expect("sample interval ms")
                )
            );
            assert_eq!(sample_interval.source, ConfigParameterSource::Default);
            // Workflow #375 R5c: a file without the dedup switches (the v0.11.4 form) keeps the
            // defaults, error windows on and Lab output off; each named key is applied.
            for (key, expected, effective) in [
                (
                    "frame_retention_dedup_error",
                    configured.is_none(),
                    assembly.host.frame_retention_dedup_error(),
                ),
                (
                    "frame_retention_dedup_lab",
                    configured.is_some(),
                    assembly.host.frame_retention_dedup_lab(),
                ),
            ] {
                let parameter = parameter(key).expect("dedup switch parameter");
                assert_eq!(parameter.value, FactScalar::Boolean(expected));
                assert_eq!(effective, expected);
                assert_eq!(parameter.source, source);
            }
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
            assert_eq!(retention.outcome, Outcome::new(reason));
            let parameter = assembly
                .manifest
                .parameters
                .iter()
                .find(|parameter| parameter.key == "frame_retention_enabled")
                .expect("frame retention parameter");
            assert_eq!(parameter.value, FactScalar::Boolean(enabled));
            assert_eq!(parameter.source, source);
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
            assert_eq!(config.assemble().err().map(Code::as_str), Some(code));
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
            config.assemble().err().map(Code::as_str),
            Some("application_identity_missing")
        );
    }

    /// Workflow #369 S6a (model 369 v3.1 H-5, #643's test): only a discovery-bound instance has
    /// an emulator the daemon can start, so `start_emulator: true` on an instance bound by its
    /// port is refused with `start_emulator_requires_discovery_binding`, naming the instance.
    #[test]
    fn start_emulator_requires_a_discovery_binding() {
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
                "application_id": "neutral.application",
                "adb_path": "adb",
                "port": 16384,
                "touch_backend": "maatouch",
                "capture_backend": "adb",
                "start_emulator": true
            }]
        });
        let config = serde_json::from_value::<ActingdConfigFile>(value).expect("typed config");
        assert_eq!(
            config.start_emulator_refusal().as_deref(),
            Some("neutral.instance")
        );
        assert_eq!(
            config.assemble().err().map(Code::as_str),
            Some("start_emulator_requires_discovery_binding")
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

    // Workflow #382 L-5b (apps gate 46): the failed-run (K/T) keys are retired. A configuration
    // that still names either one is refused as an unknown member when it is loaded, by a start
    // and by check-config alike, before anything is assembled (Fail Loud). The key table has no
    // row for either key, so an instruction that names one is `config_key_unknown`.
    #[test]
    fn l5b_a_configuration_naming_a_retired_failed_run_key_is_refused_as_unknown() {
        let root = TempDir::new().expect("tempdir");
        let path = root.path().join("actingd.config.json");
        let base = json!({
            "schema_version": CONFIG_SCHEMA_VERSION,
            "state_root": "state",
            "bind_host": "127.0.0.1",
            "secret_fingerprint_salt": "0123456789abcdef",
            "instances": []
        });
        let purposes = [
            actingcommand_contract::InstallConfigPurpose::Running,
            actingcommand_contract::InstallConfigPurpose::CandidateCheck,
        ];
        fs::write(
            &path,
            serde_json::to_vec(&base).expect("configuration JSON"),
        )
        .expect("write the configuration");
        for purpose in purposes {
            assert!(
                load_for(&path, purpose).is_ok(),
                "{purpose:?}: the configuration without a retired key loads"
            );
        }
        // Each key with the value it defaulted to before its retirement (K = 3, T = 7).
        for (key, value) in [
            ("frame_retention_failed_run_successes", 3),
            ("frame_retention_failed_run_days", 7),
        ] {
            let mut named = base.clone();
            named[key] = json!(value);
            fs::write(
                &path,
                serde_json::to_vec(&named).expect("configuration JSON"),
            )
            .expect("write the configuration");
            for purpose in purposes {
                assert_eq!(
                    load_for(&path, purpose).err(),
                    Some(SharedCode::ConfigDecodeFailed.code()),
                    "{purpose:?}: {key}"
                );
            }
            assert!(
                actingcommand_runtime_config::find_row(key).is_none(),
                "{key} has no key table row"
            );
        }
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
            config.assemble().err().map(Code::as_str),
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
            config.assemble().err().map(Code::as_str),
            Some("capture_backend_must_be_explicit")
        );
    }

    /// #398 R1 RD-3i-T5: `capture_backend: scrcpy` on an emulator instance (no `device`
    /// member) is refused as `device_config_invalid`.
    #[test]
    fn scrcpy_capture_on_an_emulator_instance_is_refused() {
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
                "capture_backend": "scrcpy"
            }]
        });
        let config = serde_json::from_value::<ActingdConfigFile>(value).expect("typed config");
        assert_eq!(
            config.assemble().err().map(Code::as_str),
            Some(ActingdCode::DeviceConfigInvalid.as_str())
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
        assert_eq!(
            config.assemble().err().map(Code::as_str),
            Some("fixture_backend_invalid")
        );
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
        // Workflow #360: a vision root without ONNX Runtime, and one whose models folder is
        // empty. Nothing is loaded at startup, so the library is an empty placeholder.
        let model = root
            .path()
            .join("no-runtime")
            .join("models")
            .join("neutral.model");
        fs::create_dir_all(&model).expect("model folder fixture");
        fs::write(model.join("rec.onnx"), b"model").expect("recognizer fixture");
        fs::write(model.join("keys.txt"), b"a").expect("dictionary fixture");
        fs::create_dir_all(root.path().join("no-models").join("models")).expect("models fixture");
        fs::create_dir_all(root.path().join("no-models").join("ort")).expect("runtime fixture");
        fs::write(
            root.path()
                .join("no-models")
                .join("ort")
                .join("onnxruntime.dll"),
            b"",
        )
        .expect("runtime placeholder");
        // Ledger revision 3 (acceptance CA-M1): a `models` entry that is a file, so listing it
        // fails with an OS error.
        fs::create_dir_all(root.path().join("models-file")).expect("vision root fixture");
        fs::write(root.path().join("models-file").join("models"), b"").expect("models file");
        let vision = |root: &str| json!({"execution_provider": "cpu", "root": root});
        // Each refusal with the `io_op` of the `foreign_os_error` link it carries, if any.
        for (index, vision, expected, os_link) in [
            (
                0,
                Some(vision("missing-root")),
                Some("vision_root_unavailable"),
                Some("canonicalize"),
            ),
            (
                1,
                Some(vision("no-runtime")),
                Some("vision_runtime_unavailable"),
                None,
            ),
            (
                2,
                Some(vision("no-models")),
                Some("vision_models_empty"),
                None,
            ),
            (
                4,
                Some(vision("models-file")),
                Some("vision_root_unavailable"),
                Some("read_dir"),
            ),
            (3, None, None, None),
        ] {
            let state_root = root.path().join(format!("state-{index}"));
            let mut config = serde_json::from_value::<ActingdConfigFile>(json!({
                "schema_version": CONFIG_SCHEMA_VERSION,
                "state_root": state_root,
                "bind_host": "127.0.0.1",
                "secret_fingerprint_salt": "0123456789abcdef",
                "vision": vision,
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
                // Ledger revision 3: the native failure is the refusal's outcome; the OS text
                // of an unavailable root is its foreign link, never public.
                assert_eq!(Some(failure.outcome.code().as_str()), expected);
                // The OS error behind a refusal is its `caused_by` link, as `foreign_os_error`
                // with the failed I/O verb.
                let os_links = failure
                    .outcome
                    .causes()
                    .iter()
                    .filter(|link| link.outcome().code().as_str() == "foreign_os_error")
                    .map(|link| {
                        link.outcome()
                            .value("io_op")
                            .and_then(serde_json::Value::as_str)
                    })
                    .collect::<Vec<_>>();
                assert_eq!(os_links, os_link.map(Some).into_iter().collect::<Vec<_>>());
                let public =
                    serde_json::to_string(&provider_terminal.0.payload().public_projection())
                        .expect("public projection");
                for link in failure.outcome.causes() {
                    if let Some(text) = link
                        .outcome()
                        .value("raw_text")
                        .and_then(serde_json::Value::as_str)
                    {
                        assert!(!public.contains(text));
                    }
                }
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
            config.assemble().err().map(Code::as_str),
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
            config.assemble().err().map(Code::as_str),
            Some("agent_dispatcher_config_invalid")
        );
    }

    /// Workflow #385 P-c, Tc-1 at start (PERF §10.3; P-c row: `with_performance_control` takes
    /// the configuration): the host's controller takes `performance.pacing.clock_jump_ms` and
    /// `.heavy_dispatch_limit.*` in effect, the model's defaults (30 000 ms, 2, 1) when the file
    /// names none, the file's values otherwise; each row says where its value came from.
    #[test]
    fn tc_1_the_controller_takes_the_performance_section_s_clock_jump_and_dispatch_limits() {
        let id = IdentifierIssuer::new()
            .expect("issuer")
            .mint_instance_id()
            .expect("instance id");
        let base = json!({
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
        let mut named = base.clone();
        named["performance"] = json!({
            "pacing": {
                "clock_jump_ms": 45_000,
                "heavy_dispatch_limit": {"normal": 3, "paused": 2}
            }
        });
        for (value, clock_jump_ms, normal, paused, source) in [
            (base, 30_000, 2, 1, ConfigParameterSource::Default),
            (named, 45_000, 3, 2, ConfigParameterSource::Explicit),
        ] {
            let config = serde_json::from_value::<ActingdConfigFile>(value).expect("typed config");
            let assembly = config.assemble().expect("runtime assembly");
            let control = assembly.host.performance_control();
            assert_eq!(
                control.clock_jump_threshold(),
                Duration::from_millis(clock_jump_ms)
            );
            assert_eq!(control.normal_heavy_dispatch_limit(), normal);
            assert_eq!(control.pressured_heavy_dispatch_limit(), paused);
            for (key, reported) in [
                (pacing::CLOCK_JUMP_MS, FactScalar::DurationMs(clock_jump_ms)),
                (
                    pacing::HEAVY_DISPATCH_LIMIT_NORMAL,
                    FactScalar::Integer(i64::from(normal)),
                ),
                (
                    pacing::HEAVY_DISPATCH_LIMIT_PAUSED,
                    FactScalar::Integer(i64::from(paused)),
                ),
            ] {
                let parameter = assembly
                    .manifest
                    .parameters
                    .iter()
                    .find(|parameter| parameter.key == key)
                    .unwrap_or_else(|| panic!("no parameter {key}"));
                assert_eq!(parameter.value, reported, "{key}");
                assert_eq!(parameter.source, source, "{key}");
            }
        }
    }

    // Workflow #385 PM-CFG (wave-17 ruling 10; PERF §10.1; register §5 "Read by P-m"): the
    // assembly hands the monitor, its sampler and the controller every key of the monitor's row
    // in effect: §10.1's defaults when the file names none, which are the constants the monitor
    // and the controller ran with before, and the file's values when it names them. A value of
    // the row that breaks one of its rules refuses the assembly as perf_config_invalid naming
    // the key.
    #[test]
    fn pmcfg_the_monitor_and_the_controller_take_every_key_of_the_monitor_row() {
        use actingcommand_runtime_config::ConfigCode;
        use actingcommand_runtime_config::performance::monitor::{
            GpuAggregate, MonitorSettings, PressureSeverity, PressureThresholds,
        };
        let id = IdentifierIssuer::new()
            .expect("issuer")
            .mint_instance_id()
            .expect("instance id");
        let base = json!({
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
        let mut named = base.clone();
        named["performance"] = json!({
            "monitor": {
                "sample_interval_ms": 3_000, "context_window_ms": 45_000,
                "summary_interval_ms": 90_000, "max_pipeline_samples": 2_048,
                "process_coverage_min_bp": 9_800, "max_system_samples": 32,
                "max_consecutive_failures": 5, "control_freshness_multiplier": 3,
                "stutter_frame_gap_ms": 800
            },
            "load": {"heavy_threshold_milli": 1_200},
            "pressure_start_samples": 4,
            "pressure_end_samples": 5,
            "pressure": {
                "thresholds": {"cpu": [7_500, 6_000], "ram": [8_000, 7_000],
                               "gpu": [8_000, 6_500], "disk_queue_milli": [2_500, 1_500],
                               "disk_latency_us": [30_000, 20_000],
                               "process_cpu_bp": 3_000, "process_io_bps": 33_554_432},
                "severity": {"critical_bp": 9_400, "elevated_bp": 8_800,
                             "third_party_critical": [4, 9_000],
                             "third_party_elevated": [3, 7_000]},
                "gpu_aggregate": "sum"
            }
        });
        // §10.1's defaults.
        let defaults = MonitorSettings {
            sample_interval_ms: 2_000,
            context_window_ms: 30_000,
            summary_interval_ms: 60_000,
            max_pipeline_samples: 4_096,
            process_coverage_min_bp: 9_900,
            max_system_samples: 64,
            max_consecutive_failures: 3,
            control_freshness_multiplier: 2,
            stutter_frame_gap_ms: 1_000,
            heavy_threshold_milli: 1_500,
            pressure_start_samples: 3,
            pressure_end_samples: 3,
            thresholds: PressureThresholds {
                cpu_bp: (8_500, 7_000),
                ram_bp: (9_000, 8_000),
                gpu_bp: (9_000, 7_500),
                disk_queue_milli: (4_000, 2_000),
                disk_latency_us: (50_000, 25_000),
                process_cpu_bp: 4_000,
                process_io_bps: 67_108_864,
            },
            severity: PressureSeverity {
                critical_bp: 9_700,
                elevated_bp: 9_200,
                third_party_critical: (3, 8_000),
                third_party_elevated: (2, 6_000),
            },
            gpu_aggregate: GpuAggregate::MaxEngineType,
        };
        // The values the file names.
        let configured = MonitorSettings {
            sample_interval_ms: 3_000,
            context_window_ms: 45_000,
            summary_interval_ms: 90_000,
            max_pipeline_samples: 2_048,
            process_coverage_min_bp: 9_800,
            max_system_samples: 32,
            max_consecutive_failures: 5,
            control_freshness_multiplier: 3,
            stutter_frame_gap_ms: 800,
            heavy_threshold_milli: 1_200,
            pressure_start_samples: 4,
            pressure_end_samples: 5,
            thresholds: PressureThresholds {
                cpu_bp: (7_500, 6_000),
                ram_bp: (8_000, 7_000),
                gpu_bp: (8_000, 6_500),
                disk_queue_milli: (2_500, 1_500),
                disk_latency_us: (30_000, 20_000),
                process_cpu_bp: 3_000,
                process_io_bps: 33_554_432,
            },
            severity: PressureSeverity {
                critical_bp: 9_400,
                elevated_bp: 8_800,
                third_party_critical: (4, 9_000),
                third_party_elevated: (3, 7_000),
            },
            gpu_aggregate: GpuAggregate::Sum,
        };
        for (value, expected) in [(base.clone(), defaults), (named, configured)] {
            let config = serde_json::from_value::<ActingdConfigFile>(value).expect("typed config");
            let assembly = config.assemble().expect("runtime assembly");
            assert_eq!(
                assembly.host.performance_monitor(),
                Some(&PerformanceMonitorConfig::default().with_settings(&expected))
            );
            assert_eq!(
                assembly.host.performance_control().heavy_threshold_milli(),
                expected.heavy_threshold_milli
            );
        }
        // The defaults are the constants the monitor and the controller ran with before.
        assert_eq!(
            PerformanceMonitorConfig::default().with_settings(&defaults),
            PerformanceMonitorConfig::default()
        );
        assert_eq!(
            PerformanceControlConfig::default().heavy_threshold_milli(),
            defaults.heavy_threshold_milli
        );

        // A context window shorter than the sampling period.
        let mut broken = base;
        broken["performance"] = json!({"monitor": {"context_window_ms": 1_500}});
        let config = serde_json::from_value::<ActingdConfigFile>(broken).expect("typed config");
        let Err((code, refusal)) = config.assemble_or_refusal() else {
            panic!("the assembly is refused");
        };
        assert_eq!(code, ConfigCode::PerfConfigInvalid.code());
        assert!(refusal.is(code));
        assert_eq!(
            refusal.value("config_key"),
            Some(&json!("performance.monitor.context_window_ms"))
        );
    }
}
