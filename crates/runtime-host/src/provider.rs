// SPDX-License-Identifier: AGPL-3.0-only

#[cfg(feature = "fixture-backends")]
mod fixtures;

use crate::{RuntimeHostError, RuntimeHostResult};
use actingcommand_contract::{
    ApplicationLifecycleAction, EmulatorCapability, EmulatorCapabilityAvailability,
    EmulatorCapabilityEvidence, EmulatorCapabilityImplementation, EmulatorCapabilityProfile,
    EmulatorInstanceAction, EmulatorVersionEvidence, InstanceId, MAX_INSTANCE_ALIAS_BYTES,
    RuntimeErrorCode,
};
use actingcommand_device::{
    Adb, AdbConfig, CaptureBackend, CaptureBackendChoice, CaptureBackendConfig, DeviceError,
    DeviceErrorCategory, DeviceErrorDiagnosticMessage, DeviceErrorSensitivity, DeviceResult,
    DeviceTarget, InputBackend, MumuDiscoveryReport, NemuAppIndex, NemuApplicationTarget,
    NemuInputConfig, NemuIpcSession, NemuResolutionReason, NemuSessionBackends,
    PreparedSegmentedSwipePlan, TouchBackendChoice, TouchBackendConfig,
    create_touch_backend_for_fenced_input, discover_mumu_instances, mumu_state_wait,
};
pub use actingcommand_execution_kernel::{
    DiscoveredInstanceBinding, EmulatorControlFailure, EmulatorControlOutcome,
    EmulatorControlResult, ExecutionBackendProvider, ForegroundApplicationObservation,
    InstanceDiscoveryFailure, PendingAdbEndpoint, ProviderDiscoveredInstance,
    ProviderInstanceDiscovery, RecognitionVisionProvider, ResolvedAdbEndpoint,
    ResolvedExecutionInstance, ResolvedInstanceEndpoint, VisionFfiProvider, VisionModelIdentity,
};
#[cfg(feature = "fixture-backends")]
pub use fixtures::FixtureInstanceSpec;
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Duration;

/// Placeholder port of a pending discovery binding: never dispatched, because every path that
/// opens a session refuses a pending entry first, and always overwritten by the rebind.
const PENDING_ADB_PORT: u16 = 0;

/// The typed, serde-free assembly the daemon hands the registry: every instance it registers,
/// the vision provider it injects and the discovery surface it exposes. Built by the daemon
/// from its configuration; consumed once by [`ExecutionBackendRegistry::from_assembly`].
pub struct ProviderAssembly {
    pub instances: Vec<InstanceSpec>,
    pub vision: Option<VisionSpec>,
    pub discovery: Option<DiscoverySpec>,
}

/// One instance to register: its alias, its identity and the backends behind it.
pub struct InstanceSpec {
    alias: String,
    instance_id: InstanceId,
    mode: InstanceMode,
}

pub enum InstanceMode {
    /// A physical device or emulator reached through ADB (and optionally Nemu IPC).
    Real(Box<ExecutionBackendRegistration>),
    /// A device-free fixture replaying configured frames.
    #[cfg(feature = "fixture-backends")]
    Fixture(FixtureInstanceSpec),
}

impl InstanceSpec {
    /// A real instance; alias and identity are the registration's.
    pub fn real(registration: ExecutionBackendRegistration) -> Self {
        Self {
            alias: registration.instance_alias.clone(),
            instance_id: registration.instance_id,
            mode: InstanceMode::Real(Box::new(registration)),
        }
    }

    /// A fixture instance under the same alias rules as a real one.
    #[cfg(feature = "fixture-backends")]
    pub fn fixture(
        alias: impl Into<String>,
        instance_id: InstanceId,
        fixture: FixtureInstanceSpec,
    ) -> RuntimeHostResult<Self> {
        let alias = alias.into();
        validate_alias(&alias)?;
        Ok(Self {
            alias,
            instance_id,
            mode: InstanceMode::Fixture(fixture),
        })
    }

    pub fn alias(&self) -> &str {
        &self.alias
    }

    pub const fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    pub const fn mode(&self) -> &InstanceMode {
        &self.mode
    }
}

/// The vision provider injected into the registry, already constructed by provider startup.
pub struct VisionSpec {
    provider: Arc<dyn RecognitionVisionProvider>,
}

impl VisionSpec {
    pub fn new(provider: Arc<dyn RecognitionVisionProvider>) -> Self {
        Self { provider }
    }
}

/// The provider's instance-discovery surface: `MuMuManager` discovery from an explicit
/// install root or, without one, from the resolver's own sources.
pub struct DiscoverySpec {
    mumu_root: Option<PathBuf>,
    /// The caller-injected `ACTINGCOMMAND_NEMU_FOLDER` value (Workflow #318 cfg3); the
    /// resolver's environment rung takes part only when it is set.
    env_nemu_folder: Option<PathBuf>,
}

impl DiscoverySpec {
    pub fn new(mumu_root: Option<PathBuf>) -> Self {
        Self {
            mumu_root,
            env_nemu_folder: None,
        }
    }

    /// Injects the `ACTINGCOMMAND_NEMU_FOLDER` fallback; `None` leaves that rung out.
    pub fn with_env_nemu_folder(mut self, env_nemu_folder: Option<PathBuf>) -> Self {
        self.env_nemu_folder = env_nemu_folder;
        self
    }

    pub fn mumu_root(&self) -> Option<&Path> {
        self.mumu_root.as_deref()
    }

    /// One `MuMuManager` discovery run, its refusal classified as the provider view reports
    /// it: a version below the policy floor or unparseable is
    /// `mumu_manager_version_unsupported`, anything else (no install, tool spawn, exit,
    /// timeout, decode or JSON failure) `instance_discovery_unavailable`. Binds, rebinds and
    /// registers nothing.
    pub fn discover(&self) -> Result<MumuDiscoveryReport, Box<InstanceDiscoveryFailure>> {
        let env_nemu_folder = self.env_nemu_folder.as_deref();
        discover_mumu_instances(self.mumu_root.as_deref(), env_nemu_folder).map_err(|error| {
            let code = if matches!(
                error
                    .nemu_resolution_context()
                    .map(|context| context.reason()),
                Some(
                    NemuResolutionReason::ProviderVersionBelowMinimum
                        | NemuResolutionReason::ProviderVersionUnparseable
                )
            ) {
                "mumu_manager_version_unsupported"
            } else {
                "instance_discovery_unavailable"
            };
            Box::new(InstanceDiscoveryFailure { code, error })
        })
    }
}

pub struct ExecutionBackendRegistration {
    instance_alias: String,
    instance_id: InstanceId,
    application_id: String,
    input: TouchBackendConfig,
    capture: CaptureBackendConfig,
    configuration: actingcommand_contract::EffectiveDeviceConfiguration,
    discovered: Option<DiscoveredInstanceBinding>,
    /// Discovery reported the instance stopped: the target carries no port yet.
    endpoint_pending: bool,
    provider_profile: Option<EmulatorCapabilityProfile>,
    nemu_app_index: Option<NemuAppIndex>,
}

impl ExecutionBackendRegistration {
    pub fn new(
        instance_alias: impl Into<String>,
        instance_id: InstanceId,
        application_id: impl Into<String>,
        input: TouchBackendConfig,
        capture: CaptureBackendConfig,
    ) -> RuntimeHostResult<Self> {
        let instance_alias = instance_alias.into();
        let application_id = application_id.into();
        validate_alias(&instance_alias)?;
        validate_application_id(&application_id)?;
        if matches!(
            input.requested,
            TouchBackendChoice::Auto | TouchBackendChoice::AutoFastest
        ) || matches!(
            capture.requested,
            CaptureBackendChoice::Auto | CaptureBackendChoice::AutoFastest
        ) {
            return Err(RuntimeHostError::fatal(
                "execution_backend_selection_not_explicit",
                "build_execution_backend_registry",
                RuntimeErrorCode::RuntimeFatal,
            ));
        }
        if input.target.resolved_serial() != capture.target.resolved_serial() {
            return Err(RuntimeHostError::fatal(
                "execution_backend_target_mismatch",
                "build_execution_backend_registry",
                RuntimeErrorCode::RuntimeFatal,
            ));
        }
        if input.requested == TouchBackendChoice::NemuIpc
            && capture.requested != CaptureBackendChoice::NemuIpc
        {
            return Err(RuntimeHostError::fatal(
                "nemu_input_requires_paired_capture",
                "build_execution_backend_registry",
                RuntimeErrorCode::RuntimeFatal,
            ));
        }
        let milliseconds = |duration: Duration| {
            u64::try_from(duration.as_millis()).map_err(|_| {
                RuntimeHostError::fatal(
                    "execution_configuration_timeout_overflow",
                    "build_execution_backend_registry",
                    RuntimeErrorCode::RuntimeFatal,
                )
            })
        };
        let configuration = actingcommand_contract::EffectiveDeviceConfiguration {
            input_backend: input.requested.as_str().to_owned(),
            capture_backend: capture.requested.as_str().to_owned(),
            input_adb: input.adb_config.adb_path.clone(),
            capture_adb: capture.adb_config.adb_path.clone(),
            configured_serial: input.target.serial.clone(),
            resolved_serial: input.target.resolved_serial(),
            input_command_timeout_ms: milliseconds(input.adb_config.command_timeout)?,
            capture_command_timeout_ms: milliseconds(capture.adb_config.command_timeout)?,
            capture_timeout_ms: milliseconds(capture.capture_timeout)?,
            configured_mumu_root: capture.nemu.nemu_folder.clone(),
            configured_capture_dll: capture.nemu.dll_path.clone(),
        };
        Ok(Self {
            instance_alias,
            instance_id,
            application_id,
            input,
            capture,
            configuration,
            discovered: None,
            endpoint_pending: false,
            provider_profile: None,
            nemu_app_index: None,
        })
    }

    /// Marks the registration as bound through MuMu instance discovery.
    pub fn with_discovered_binding(mut self, discovered: DiscoveredInstanceBinding) -> Self {
        self.discovered = Some(discovered);
        self.endpoint_pending = false;
        self
    }

    /// Marks the registration as discovered stopped: it is registered with a pending endpoint
    /// (no port) and bound once emulator control starts the instance.
    pub fn with_pending_discovered_binding(
        mut self,
        discovered: DiscoveredInstanceBinding,
    ) -> Self {
        self.discovered = Some(discovered);
        self.endpoint_pending = true;
        self
    }

    /// Attaches the provider capability profile admitted at startup; its provider-owned rows
    /// replace the registry placeholder rows when the registration is registered.
    pub fn with_capability_profile(mut self, profile: EmulatorCapabilityProfile) -> Self {
        self.provider_profile = Some(profile);
        self
    }

    pub fn with_nemu_app_index(mut self, app_index: NemuAppIndex) -> RuntimeHostResult<Self> {
        if self.input.requested != TouchBackendChoice::NemuIpc
            || self.capture.requested != CaptureBackendChoice::NemuIpc
        {
            return Err(RuntimeHostError::fatal(
                "nemu_app_index_requires_paired_input",
                "build_execution_backend_registry",
                RuntimeErrorCode::RuntimeFatal,
            ));
        }
        NemuApplicationTarget::new(&self.application_id, app_index).map_err(|error| {
            RuntimeHostError::fatal(
                "nemu_application_identity_invalid",
                "build_execution_backend_registry",
                RuntimeErrorCode::RuntimeFatal,
            )
            .with_native_detail(error.to_string())
        })?;
        self.nemu_app_index = Some(app_index);
        Ok(self)
    }
}

enum RegistryEntry {
    Real(Box<ExecutionBackendEntry>),
    #[cfg(feature = "fixture-backends")]
    Fixture(fixtures::FixtureEntry),
}

impl RegistryEntry {
    const fn instance_id(&self) -> InstanceId {
        match self {
            Self::Real(entry) => entry.instance_id,
            #[cfg(feature = "fixture-backends")]
            Self::Fixture(fixture) => fixture.instance_id(),
        }
    }
}

struct ExecutionBackendEntry {
    instance_id: InstanceId,
    application_id: String,
    application_adb: AdbConfig,
    capabilities: EmulatorCapabilityProfile,
    nemu_app_index: Option<NemuAppIndex>,
    /// The ADB endpoint and everything built on it, behind a lock because emulator control
    /// (re)binds a discovery-bound entry. Every guarded section only copies plain data.
    endpoint: Mutex<EntryEndpoint>,
}

struct EntryEndpoint {
    state: ResolvedInstanceEndpoint,
    audit_endpoint: String,
    application_target: DeviceTarget,
    input: TouchBackendConfig,
    capture: CaptureBackendConfig,
    configuration: actingcommand_contract::EffectiveDeviceConfiguration,
}

impl EntryEndpoint {
    /// The typed refusal of every session-opening path while the port is unknown.
    fn require_bound(&self, operation: &'static str) -> DeviceResult<()> {
        if self.state.is_pending() {
            return Err(DeviceError::fatal(
                "adb endpoint pending: the discovered instance was stopped and has reported no ADB port; start it through emulator control first",
            )
            .with_diagnostic(DeviceErrorCategory::Protocol, "adb.endpoint_pending")
            .with_diagnostic_context(
                "execution_backend_registry",
                operation,
                DeviceErrorSensitivity::Sensitive,
            ));
        }
        Ok(())
    }

    /// Rewrites every port-dependent field for the given target.
    fn set_target(&mut self, host: &str, port: u16) {
        for target in [
            &mut self.application_target,
            &mut self.input.target,
            &mut self.capture.target,
        ] {
            target.host = host.to_owned();
            target.port = port;
        }
        self.audit_endpoint = self.application_target.resolved_serial();
        self.configuration.resolved_serial = self.audit_endpoint.clone();
    }
}

impl ExecutionBackendEntry {
    fn new(registration: ExecutionBackendRegistration) -> RuntimeHostResult<Self> {
        let state = match (registration.discovered, registration.endpoint_pending) {
            (Some(discovered), true) => ResolvedInstanceEndpoint::Pending(PendingAdbEndpoint::new(
                registration.input.target.host.clone(),
                discovered,
            )),
            (discovered, _) => {
                let mut adb_endpoint = ResolvedAdbEndpoint::new(
                    registration.input.target.host.clone(),
                    registration.input.target.port,
                    registration.input.target.serial.is_some(),
                );
                if let Some(discovered) = discovered {
                    adb_endpoint = adb_endpoint.with_discovered_binding(discovered);
                }
                ResolvedInstanceEndpoint::Bound(adb_endpoint)
            }
        };
        let application_adb = registration.input.adb_config.clone();
        let application_target = registration.input.target.clone();
        let capabilities = capability_profile(&registration.input, &registration.capture)?;
        let capabilities = match registration.provider_profile {
            Some(provider) => merge_capability_profile(&capabilities, &provider)?,
            None => capabilities,
        };
        let mut endpoint = EntryEndpoint {
            state,
            audit_endpoint: application_target.resolved_serial(),
            application_target,
            input: registration.input,
            capture: registration.capture,
            configuration: registration.configuration,
        };
        if endpoint.state.is_pending() {
            let host = endpoint.application_target.host.clone();
            endpoint.set_target(&host, PENDING_ADB_PORT);
        }
        Ok(Self {
            instance_id: registration.instance_id,
            application_id: registration.application_id,
            application_adb,
            capabilities,
            nemu_app_index: registration.nemu_app_index,
            endpoint: Mutex::new(endpoint),
        })
    }

    /// The guarded sections never panic, so a poisoned lock still holds a consistent state.
    fn endpoint(&self) -> MutexGuard<'_, EntryEndpoint> {
        self.endpoint.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn resolve(&self) -> ResolvedExecutionInstance {
        let (state, audit_endpoint, configuration) = {
            let endpoint = self.endpoint();
            (
                endpoint.state.clone(),
                endpoint.audit_endpoint.clone(),
                endpoint.configuration.clone(),
            )
        };
        let resolved = ResolvedExecutionInstance::new(self.instance_id, audit_endpoint)
            .with_configuration(configuration)
            .with_capabilities(self.capabilities.clone());
        match state {
            ResolvedInstanceEndpoint::Bound(adb_endpoint) => {
                resolved.with_adb_endpoint(adb_endpoint)
            }
            ResolvedInstanceEndpoint::Pending(pending) => resolved.with_pending_endpoint(pending),
        }
    }

    fn open_input(
        &self,
    ) -> DeviceResult<actingcommand_device::OpenedBackend<Box<dyn InputBackend>>> {
        let input = {
            let endpoint = self.endpoint();
            endpoint.require_bound("open_input")?;
            endpoint.input.clone()
        };
        let requested = input.requested;
        let serial_configured = input.target.serial.is_some();
        let mut report = actingcommand_contract::BackendOpenReport::unobserved(
            actingcommand_contract::BackendOpenEntry::Input,
        );
        report.source = actingcommand_contract::BackendOpenSource::Native;
        report.requested = requested.as_str().into();
        report.serial_configured = Some(serial_configured);
        create_touch_backend_for_fenced_input(input)
            .map(|backend| {
                let report = backend.open_report(serial_configured);
                actingcommand_device::OpenedBackend::new(
                    Box::new(DeviceRegistryInputDiagnosticBackend::new(
                        Box::new(backend),
                        requested,
                    )) as Box<dyn InputBackend>,
                    report,
                )
            })
            .map_err(|error| actingcommand_device::observe_open_failure(report, error))
    }

    fn open_capture(
        &self,
        memory: Option<&actingcommand_device::FrameMemoryBudget>,
    ) -> DeviceResult<actingcommand_device::OpenedBackend<Box<dyn CaptureBackend>>> {
        let capture = {
            let endpoint = self.endpoint();
            endpoint.require_bound("open_capture")?;
            endpoint.capture.clone()
        };
        let mut report = actingcommand_contract::BackendOpenReport::unobserved(
            actingcommand_contract::BackendOpenEntry::Capture,
        );
        report.source = actingcommand_contract::BackendOpenSource::Native;
        report.requested = capture.requested.as_str().into();
        report.serial_configured = Some(capture.target.serial.is_some());
        report.installation_source = capture
            .resolved_mumu
            .as_ref()
            .map(|context| context.source.into());
        actingcommand_device::create_capture_backend_with_memory(capture, memory)
            .map(|selected| {
                let report = selected.open_report();
                actingcommand_device::OpenedBackend::new(
                    Box::new(selected) as Box<dyn CaptureBackend>,
                    report,
                )
            })
            .map_err(|error| actingcommand_device::observe_open_failure(report, error))
    }

    fn open_nemu_session(
        &self,
    ) -> DeviceResult<Option<actingcommand_device::OpenedBackend<NemuSessionBackends>>> {
        // Same pending guard as `open_input` / `open_capture`: a paired session is opened on
        // the bound target only.
        let (input, capture) = {
            let endpoint = self.endpoint();
            if endpoint.input.requested != TouchBackendChoice::NemuIpc {
                return Ok(None);
            }
            endpoint.require_bound("open_nemu_session")?;
            (endpoint.input.clone(), endpoint.capture.clone())
        };
        let application = NemuApplicationTarget::new(
            &self.application_id,
            self.nemu_app_index
                .ok_or_else(|| DeviceError::fatal("Nemu application index is missing"))?,
        )?;
        NemuIpcSession::open(
            capture,
            application,
            NemuInputConfig {
                command_timeout: input.adb_config.command_timeout,
                shutdown_timeout: input.maatouch_config.shutdown_timeout,
                tap_hold: input.maatouch_config.tap_hold,
            },
        )
        .map_err(|error| {
            let mut report = actingcommand_contract::BackendOpenReport::unobserved(
                actingcommand_contract::BackendOpenEntry::NemuPair,
            );
            report.source = actingcommand_contract::BackendOpenSource::Native;
            report.requested = "nemu_ipc".into();
            actingcommand_device::observe_open_failure(report, error)
        })
        .map(|mut pair| {
            pair.backend.input = Box::new(DeviceRegistryInputDiagnosticBackend::new(
                pair.backend.input,
                TouchBackendChoice::NemuIpc,
            ));
            Some(pair)
        })
    }

    fn control_application(&self, action: ApplicationLifecycleAction) -> DeviceResult<()> {
        let application_target = {
            let endpoint = self.endpoint();
            endpoint.require_bound("control_application")?;
            endpoint.application_target.clone()
        };
        let serial = application_target.resolved_serial();
        let adb = Adb::new(self.application_adb.clone());
        adb.ensure_device(&serial, application_target.connect)?;
        match action {
            ApplicationLifecycleAction::Launch => {
                adb.launch_package(&serial, &self.application_id)?;
            }
            ApplicationLifecycleAction::Stop => {
                adb.force_stop(&serial, &self.application_id)?;
            }
            ApplicationLifecycleAction::Restart => {
                adb.force_stop(&serial, &self.application_id)?;
                thread::sleep(Duration::from_millis(500));
                adb.launch_package(&serial, &self.application_id)?;
            }
        }
        Ok(())
    }

    /// The ADB baseline probe: the bound endpoint's `ensure_device` with a connect attempt
    /// allowed (a freshly started emulator answers `adb connect` a few seconds after the
    /// vendor reports it running). No session is opened.
    fn probe_adb_baseline(&self) -> DeviceResult<()> {
        let application_target = {
            let endpoint = self.endpoint();
            endpoint.require_bound("probe_adb_baseline")?;
            endpoint.application_target.clone()
        };
        Adb::new(self.application_adb.clone())
            .ensure_device(&application_target.resolved_serial(), true)
            .map(|_| ())
    }

    fn probe_adb_baseline_until(
        &self,
        deadline: std::time::Instant,
        stopped: &dyn Fn() -> bool,
    ) -> DeviceResult<()> {
        let application_target = {
            let endpoint = self.endpoint();
            endpoint.require_bound("probe_adb_baseline")?;
            endpoint.application_target.clone()
        };
        Adb::new(self.application_adb.clone())
            .ensure_device_until(
                &application_target.resolved_serial(),
                true,
                deadline,
                stopped,
            )
            .map(|_| ())
    }

    /// The ADB baseline query behind the foreground gate: same bound-endpoint guard and
    /// `ensure_device` as `control_application`, then one read-only `dumpsys`. No session
    /// is opened; a Nemu paired session, when one is open, is not consulted.
    fn observe_foreground_application(&self) -> DeviceResult<ForegroundApplicationObservation> {
        let application_target = {
            let endpoint = self.endpoint();
            endpoint.require_bound("observe_foreground_application")?;
            endpoint.application_target.clone()
        };
        let serial = application_target.resolved_serial();
        let adb = Adb::new(self.application_adb.clone());
        adb.ensure_device(&serial, application_target.connect)?;
        Ok(ForegroundApplicationObservation {
            foreground: adb.foreground_package(&serial)?,
            assigned: self.application_id.clone(),
        })
    }
}

/// The only [`ExecutionBackendProvider`]: every instance the daemon configured, real or
/// fixture, behind one alias set, one vision provider and one discovery surface.
pub struct ExecutionBackendRegistry {
    entries: BTreeMap<String, RegistryEntry>,
    vision_provider: Option<Arc<dyn RecognitionVisionProvider>>,
    discovery: Option<DiscoverySpec>,
}

impl ExecutionBackendRegistry {
    /// A registry of real instances only, without vision or discovery; it must not be empty.
    pub fn new(
        registrations: impl IntoIterator<Item = ExecutionBackendRegistration>,
    ) -> RuntimeHostResult<Self> {
        let mut registry = Self {
            entries: BTreeMap::new(),
            vision_provider: None,
            discovery: None,
        };
        for registration in registrations {
            registry.register(registration)?;
        }
        if registry.entries.is_empty() {
            return Err(RuntimeHostError::fatal(
                "empty_execution_backend_registry",
                "build_execution_backend_registry",
                RuntimeErrorCode::RuntimeFatal,
            ));
        }
        Ok(registry)
    }

    /// The single owner of backend assembly: registers every instance once under one
    /// alias / instance-id rule (`duplicate_instance_alias` / `duplicate_instance_id`),
    /// routes real and fixture instances, injects the vision provider once and keeps the
    /// discovery surface. Opens are wrapped once, with `observe_open_failure`.
    pub fn from_assembly(assembly: ProviderAssembly) -> RuntimeHostResult<Self> {
        let ProviderAssembly {
            instances,
            vision,
            discovery,
        } = assembly;
        let mut registry = Self {
            entries: BTreeMap::new(),
            vision_provider: vision.map(|spec| spec.provider),
            discovery,
        };
        for spec in instances {
            registry.register_spec(spec)?;
        }
        Ok(registry)
    }

    /// Adds one real registration under the same duplicate-alias and duplicate-id rules as
    /// `from_assembly`.
    pub fn register(
        &mut self,
        registration: ExecutionBackendRegistration,
    ) -> RuntimeHostResult<()> {
        self.register_spec(InstanceSpec::real(registration))
    }

    fn register_spec(&mut self, spec: InstanceSpec) -> RuntimeHostResult<()> {
        let InstanceSpec {
            alias,
            instance_id,
            mode,
        } = spec;
        if let InstanceMode::Real(registration) = &mode
            && registration.input.requested == TouchBackendChoice::NemuIpc
            && registration.nemu_app_index.is_none()
        {
            return Err(RuntimeHostError::fatal(
                "nemu_app_index_missing",
                "build_execution_backend_registry",
                RuntimeErrorCode::RuntimeFatal,
            ));
        }
        if self.entries.contains_key(&alias) {
            return Err(RuntimeHostError::fatal(
                "duplicate_instance_alias",
                "build_execution_backend_registry",
                RuntimeErrorCode::RuntimeFatal,
            ));
        }
        if self
            .entries
            .values()
            .any(|entry| entry.instance_id() == instance_id)
        {
            return Err(RuntimeHostError::fatal(
                "duplicate_instance_id",
                "build_execution_backend_registry",
                RuntimeErrorCode::RuntimeFatal,
            ));
        }
        let entry = match mode {
            InstanceMode::Real(registration) => {
                RegistryEntry::Real(Box::new(ExecutionBackendEntry::new(*registration)?))
            }
            #[cfg(feature = "fixture-backends")]
            InstanceMode::Fixture(fixture) => {
                RegistryEntry::Fixture(fixtures::FixtureEntry::new(instance_id, fixture))
            }
        };
        self.entries.insert(alias, entry);
        Ok(())
    }

    pub fn with_vision_provider(
        mut self,
        vision_provider: Arc<dyn RecognitionVisionProvider>,
    ) -> Self {
        self.vision_provider = Some(vision_provider);
        self
    }

    fn entry(&self, instance_alias: &str) -> DeviceResult<&RegistryEntry> {
        self.entries
            .get(instance_alias)
            .ok_or_else(|| DeviceError::fatal("execution backend instance is not registered"))
    }
}

/// Records implementation/configuration only; this path never opens or probes a backend.
fn capability_profile(
    input: &TouchBackendConfig,
    capture: &CaptureBackendConfig,
) -> RuntimeHostResult<EmulatorCapabilityProfile> {
    let invalid = |_| {
        RuntimeHostError::fatal(
            "execution_capability_profile_invalid",
            "build_execution_backend_registry",
            RuntimeErrorCode::RuntimeFatal,
        )
    };
    let evidence = EmulatorCapability::ALL.into_iter().map(|capability| {
        let (supported, source) = match capability {
            EmulatorCapability::InputTap | EmulatorCapability::InputLongTap
            | EmulatorCapability::InputSwipe | EmulatorCapability::InputReset =>
                (true, input.requested.as_str()),
            EmulatorCapability::InputSegmentedSwipe => (
                matches!(input.requested, TouchBackendChoice::MaaTouch | TouchBackendChoice::Minitouch),
                input.requested.as_str(),
            ),
            EmulatorCapability::InputKey | EmulatorCapability::InputText => (
                input.requested == TouchBackendChoice::MaaTouch, input.requested.as_str(),
            ),
            EmulatorCapability::CaptureFrame => (true, capture.requested.as_str()),
            EmulatorCapability::ApplicationLaunch | EmulatorCapability::ApplicationStop
            | EmulatorCapability::ApplicationRestart => (true, "adb_application_lifecycle"),
            EmulatorCapability::InventoryRead | EmulatorCapability::InstanceStatusRead
            | EmulatorCapability::InstanceStart | EmulatorCapability::InstanceStop
            | EmulatorCapability::InstanceRestart | EmulatorCapability::InstanceCreate
            | EmulatorCapability::InstanceClone | EmulatorCapability::InstanceDelete
            | EmulatorCapability::InstanceConfigure | EmulatorCapability::ApplicationControl
            | EmulatorCapability::AdbBridge | EmulatorCapability::SnapshotManage =>
                (false, "execution_backend_registry"),
        };
        let (implementation, availability, refusal) = if supported {
            (EmulatorCapabilityImplementation::Supported, EmulatorCapabilityAvailability::Unverified,
             "Implementation selected; version, assets, connection and device availability have not been verified. Execution requires the existing Runtime admission and backend checks.")
        } else {
            (EmulatorCapabilityImplementation::Unsupported, EmulatorCapabilityAvailability::Unavailable,
             "The registered backend does not implement this capability; it must not be executed through this capability claim.")
        };
        EmulatorCapabilityEvidence::new(capability, availability, refusal, source)?
            .with_implementation(implementation)
    }).collect::<Result<Vec<_>, _>>().map_err(invalid)?;
    EmulatorCapabilityProfile::new(
        "runtime.execution_backend_registry",
        EmulatorVersionEvidence::Unavailable {
            reason: "Backend versions have not been probed.".to_owned(),
        },
        evidence,
    )
    .map_err(invalid)
}

/// Provider id, version and the provider-owned rows (inventory, instance status and
/// lifecycle, instance management, snapshots, ADB bridge) come from the admitted provider
/// profile; input, capture, application lifecycle and application control keep the
/// registry-derived evidence. The result is re-validated as a complete profile.
fn merge_capability_profile(
    registry: &EmulatorCapabilityProfile,
    provider: &EmulatorCapabilityProfile,
) -> RuntimeHostResult<EmulatorCapabilityProfile> {
    let evidence = EmulatorCapability::ALL
        .into_iter()
        .map(|capability| {
            let source = match capability {
                EmulatorCapability::InventoryRead
                | EmulatorCapability::InstanceStatusRead
                | EmulatorCapability::InstanceStart
                | EmulatorCapability::InstanceStop
                | EmulatorCapability::InstanceRestart
                | EmulatorCapability::InstanceCreate
                | EmulatorCapability::InstanceClone
                | EmulatorCapability::InstanceDelete
                | EmulatorCapability::InstanceConfigure
                | EmulatorCapability::SnapshotManage
                | EmulatorCapability::AdbBridge => provider,
                EmulatorCapability::ApplicationControl
                | EmulatorCapability::InputTap
                | EmulatorCapability::InputLongTap
                | EmulatorCapability::InputSwipe
                | EmulatorCapability::InputSegmentedSwipe
                | EmulatorCapability::InputKey
                | EmulatorCapability::InputText
                | EmulatorCapability::InputReset
                | EmulatorCapability::CaptureFrame
                | EmulatorCapability::ApplicationLaunch
                | EmulatorCapability::ApplicationStop
                | EmulatorCapability::ApplicationRestart => registry,
            };
            source.evidence(capability).clone()
        })
        .collect();
    EmulatorCapabilityProfile::new(provider.provider_id(), provider.version().clone(), evidence)
        .map_err(|_| {
            RuntimeHostError::fatal(
                "execution_capability_profile_merge_invalid",
                "build_execution_backend_registry",
                RuntimeErrorCode::RuntimeFatal,
            )
        })
}

impl fmt::Debug for ExecutionBackendRegistry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExecutionBackendRegistry")
            .field("instance_count", &self.entries.len())
            .field("vision_provider", &self.vision_provider.is_some())
            .field("discovery", &self.discovery.is_some())
            .finish()
    }
}

impl ExecutionBackendProvider for ExecutionBackendRegistry {
    fn instance_aliases(&self) -> Vec<String> {
        self.entries.keys().cloned().collect()
    }

    fn resolve(&self, instance_alias: &str) -> Option<ResolvedExecutionInstance> {
        Some(match self.entries.get(instance_alias)? {
            RegistryEntry::Real(entry) => entry.resolve(),
            #[cfg(feature = "fixture-backends")]
            RegistryEntry::Fixture(fixture) => fixture.resolve(),
        })
    }

    fn open_input(
        &self,
        instance_alias: &str,
    ) -> DeviceResult<actingcommand_device::OpenedBackend<Box<dyn InputBackend>>> {
        match self.entry(instance_alias)? {
            RegistryEntry::Real(entry) => entry.open_input(),
            #[cfg(feature = "fixture-backends")]
            RegistryEntry::Fixture(fixture) => Ok(fixture.open_input()),
        }
    }

    fn open_capture(
        &self,
        instance_alias: &str,
        _memory: Option<&actingcommand_device::FrameMemoryBudget>,
    ) -> DeviceResult<actingcommand_device::OpenedBackend<Box<dyn CaptureBackend>>> {
        match self.entry(instance_alias)? {
            RegistryEntry::Real(entry) => entry.open_capture(_memory),
            #[cfg(feature = "fixture-backends")]
            RegistryEntry::Fixture(fixture) => fixture.open_capture(),
        }
    }

    fn open_nemu_session(
        &self,
        instance_alias: &str,
    ) -> DeviceResult<Option<actingcommand_device::OpenedBackend<NemuSessionBackends>>> {
        match self.entry(instance_alias)? {
            RegistryEntry::Real(entry) => entry.open_nemu_session(),
            #[cfg(feature = "fixture-backends")]
            RegistryEntry::Fixture(_) => Ok(None),
        }
    }

    fn control_application(
        &self,
        instance_alias: &str,
        action: ApplicationLifecycleAction,
    ) -> DeviceResult<()> {
        match self.entry(instance_alias)? {
            RegistryEntry::Real(entry) => entry.control_application(action),
            #[cfg(feature = "fixture-backends")]
            RegistryEntry::Fixture(_) => fixtures::FixtureEntry::control_application(),
        }
    }

    fn probe_adb_baseline(&self, instance_alias: &str) -> DeviceResult<()> {
        match self.entry(instance_alias)? {
            RegistryEntry::Real(entry) => entry.probe_adb_baseline(),
            // A fixture has no ADB baseline to wait for.
            #[cfg(feature = "fixture-backends")]
            RegistryEntry::Fixture(_) => Ok(()),
        }
    }

    fn probe_adb_baseline_until(
        &self,
        instance_alias: &str,
        deadline: std::time::Instant,
        stopped: &dyn Fn() -> bool,
    ) -> DeviceResult<()> {
        match self.entry(instance_alias)? {
            RegistryEntry::Real(entry) => entry.probe_adb_baseline_until(deadline, stopped),
            #[cfg(feature = "fixture-backends")]
            RegistryEntry::Fixture(_) => {
                fixtures::FixtureEntry::probe_adb_baseline_until(deadline, stopped)
            }
        }
    }

    fn observe_foreground_application(
        &self,
        instance_alias: &str,
    ) -> DeviceResult<ForegroundApplicationObservation> {
        match self.entry(instance_alias)? {
            RegistryEntry::Real(entry) => entry.observe_foreground_application(),
            #[cfg(feature = "fixture-backends")]
            RegistryEntry::Fixture(_) => fixtures::FixtureEntry::observe_foreground_application(),
        }
    }

    /// Drives `MuMuManager control` for a discovery-bound entry only. Explicit (non-discovered)
    /// entries carry no `MuMuManager` path and are refused with `emulator_control.unavailable`.
    /// No device session is opened or touched here.
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
                        "execution_backend_registry",
                        "control_instance",
                        DeviceErrorSensitivity::Sensitive,
                    ),
                0,
            )
        };
        let entry = match self.entries.get(instance_alias) {
            Some(RegistryEntry::Real(entry)) => entry,
            #[cfg(feature = "fixture-backends")]
            Some(RegistryEntry::Fixture(_)) => {
                return Err(refused(
                    "emulator control unavailable: a fixture simulation instance has no emulator",
                    "emulator_control.unavailable",
                ));
            }
            None => {
                return Err(refused(
                    "execution backend instance is not registered",
                    "emulator_control.unregistered",
                ));
            }
        };
        let Some(discovered) = entry.endpoint().state.discovered_binding().cloned() else {
            return Err(refused(
                "emulator control unavailable: the instance was registered explicitly, without MuMuManager discovery, so no MuMuManager executable is bound to it",
                "emulator_control.unavailable",
            ));
        };
        actingcommand_device::control_instance(
            discovered.mumu_manager_path(),
            discovered.instance_index(),
            action,
            mumu_state_wait(action),
        )
    }

    /// Binds a discovery-bound entry to the port the started instance reported (its host is
    /// the one the entry was registered with) or returns it to pending after a stop.
    fn rebind_discovered_endpoint(
        &self,
        instance_alias: &str,
        adb_port: Option<u16>,
    ) -> DeviceResult<()> {
        let refused = |message: &str, stage: &'static str| {
            DeviceError::fatal(message)
                .with_diagnostic(DeviceErrorCategory::Protocol, stage)
                .with_diagnostic_context(
                    "execution_backend_registry",
                    "rebind_discovered_endpoint",
                    DeviceErrorSensitivity::Sensitive,
                )
        };
        let entry = match self.entries.get(instance_alias) {
            Some(RegistryEntry::Real(entry)) => entry,
            #[cfg(feature = "fixture-backends")]
            Some(RegistryEntry::Fixture(_)) => {
                return Err(refused(
                    "endpoint rebinding unavailable: a fixture simulation instance has no emulator",
                    "emulator_control.unavailable",
                ));
            }
            None => {
                return Err(refused(
                    "execution backend instance is not registered",
                    "emulator_control.unregistered",
                ));
            }
        };
        let mut endpoint = entry.endpoint();
        let Some(discovered) = endpoint.state.discovered_binding().cloned() else {
            return Err(refused(
                "endpoint rebinding unavailable: the instance was registered explicitly, without MuMuManager discovery",
                "emulator_control.unavailable",
            ));
        };
        let host = endpoint.application_target.host.clone();
        match adb_port {
            Some(PENDING_ADB_PORT) => {
                return Err(refused(
                    "endpoint rebinding refused: the started instance reported ADB port 0",
                    "adb.endpoint_unresolved",
                ));
            }
            Some(port) => {
                endpoint.set_target(&host, port);
                endpoint.state = ResolvedInstanceEndpoint::Bound(
                    ResolvedAdbEndpoint::new(host, port, false).with_discovered_binding(discovered),
                );
            }
            None => {
                endpoint.set_target(&host, PENDING_ADB_PORT);
                endpoint.state =
                    ResolvedInstanceEndpoint::Pending(PendingAdbEndpoint::new(host, discovered));
            }
        }
        Ok(())
    }

    /// On-demand discovery: the same `MuMuManager` resolution as startup, mapped to the
    /// provider view. Binds, rebinds, registers and records nothing.
    fn discover_instances(
        &self,
    ) -> Result<ProviderInstanceDiscovery, Box<InstanceDiscoveryFailure>> {
        let Some(discovery) = &self.discovery else {
            return Err(Box::new(InstanceDiscoveryFailure {
                code: "instance_discovery_unavailable",
                error: DeviceError::fatal("instance discovery unsupported by this provider")
                    .with_diagnostic(
                        DeviceErrorCategory::Protocol,
                        "instance_discovery.unsupported",
                    )
                    .with_diagnostic_context(
                        "execution_backend_registry",
                        "discover_instances",
                        DeviceErrorSensitivity::Sensitive,
                    ),
            }));
        };
        let report = discovery.discover()?;
        Ok(ProviderInstanceDiscovery {
            provider_version: report.version.to_string(),
            instances: report
                .instances
                .into_iter()
                .map(|instance| ProviderDiscoveredInstance {
                    instance_index: instance.instance_index,
                    instance_name: instance.instance_name,
                    adb_host: instance.adb_host,
                    adb_port: instance.adb_port,
                    running: instance.running,
                    android_version: instance.android_version,
                })
                .collect(),
        })
    }

    fn vision_provider(&self) -> Option<Arc<dyn RecognitionVisionProvider>> {
        self.vision_provider.as_ref().map(Arc::clone)
    }
}

/// Completes the diagnostic of every input operation failure a real backend reports without
/// one (Workflow #257 DEVICE-DIAGNOSTIC-v1): category, stage, backend, operation and the
/// registry summary are added only where the producer left them absent.
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
    fn take_backend_open_observations(
        &mut self,
    ) -> Vec<actingcommand_device::BackendOpenObservation> {
        self.backend.take_backend_open_observations()
    }

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

fn validate_alias(alias: &str) -> RuntimeHostResult<()> {
    if actingcommand_contract::validate_instance_alias(alias).is_err() {
        return Err(RuntimeHostError::fatal(
            "invalid_instance_alias",
            "build_execution_backend_registry",
            RuntimeErrorCode::RuntimeFatal,
        ));
    }
    Ok(())
}

fn validate_application_id(application_id: &str) -> RuntimeHostResult<()> {
    if application_id.trim().is_empty()
        || application_id.len() > MAX_INSTANCE_ALIAS_BYTES
        || application_id.chars().any(char::is_control)
    {
        return Err(RuntimeHostError::fatal(
            "invalid_application_identity",
            "build_execution_backend_registry",
            RuntimeErrorCode::RuntimeFatal,
        ));
    }
    Ok(())
}

// The input-operation diagnostic wrapper's own criteria, moved here with the wrapper from the
// daemon (Workflow #257 DEVICE-DIAGNOSTIC-v1 / #241 segmented swipe v2).
#[cfg(test)]
mod tests {
    use super::*;
    use actingcommand_device::SegmentedSwipeAction;

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
}
