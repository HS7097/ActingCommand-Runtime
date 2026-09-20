// SPDX-License-Identifier: AGPL-3.0-only

use crate::{RuntimeHostError, RuntimeHostResult};
use actingcommand_contract::{
    ApplicationLifecycleAction, EmulatorCapability, EmulatorCapabilityAvailability,
    EmulatorCapabilityEvidence, EmulatorCapabilityImplementation, EmulatorCapabilityProfile,
    EmulatorInstanceAction, EmulatorVersionEvidence, InstanceId, MAX_INSTANCE_ALIAS_BYTES,
    RuntimeErrorCode,
};
use actingcommand_device::{
    Adb, AdbConfig, CaptureBackend, CaptureBackendChoice, CaptureBackendConfig, DeviceError,
    DeviceErrorCategory, DeviceErrorSensitivity, DeviceResult, DeviceTarget, InputBackend,
    NemuAppIndex, NemuApplicationTarget, NemuInputConfig, NemuIpcSession, NemuSessionBackends,
    TouchBackendChoice, TouchBackendConfig, create_capture_backend,
    create_touch_backend_for_fenced_input, mumu_state_wait,
};
pub use actingcommand_execution_kernel::{
    DiscoveredInstanceBinding, EmulatorControlFailure, EmulatorControlOutcome,
    EmulatorControlResult, ExecutionBackendProvider, ForegroundApplicationObservation,
    PendingAdbEndpoint, RecognitionVisionProvider, ResolvedAdbEndpoint, ResolvedExecutionInstance,
    ResolvedInstanceEndpoint, VisionFfiProvider, VisionModelIdentity,
};
use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Duration;

/// Placeholder port of a pending discovery binding: never dispatched, because every path that
/// opens a session refuses a pending entry first, and always overwritten by the rebind.
const PENDING_ADB_PORT: u16 = 0;

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
    /// The guarded sections never panic, so a poisoned lock still holds a consistent state.
    fn endpoint(&self) -> MutexGuard<'_, EntryEndpoint> {
        self.endpoint.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

pub struct ExecutionBackendRegistry {
    entries: BTreeMap<String, ExecutionBackendEntry>,
    vision_provider: Option<Arc<dyn RecognitionVisionProvider>>,
}

impl ExecutionBackendRegistry {
    pub fn new(
        registrations: impl IntoIterator<Item = ExecutionBackendRegistration>,
    ) -> RuntimeHostResult<Self> {
        let mut registry = Self {
            entries: BTreeMap::new(),
            vision_provider: None,
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

    /// Adds one registration under the same duplicate-alias and duplicate-id rules as `new`.
    pub fn register(
        &mut self,
        registration: ExecutionBackendRegistration,
    ) -> RuntimeHostResult<()> {
        if registration.input.requested == TouchBackendChoice::NemuIpc
            && registration.nemu_app_index.is_none()
        {
            return Err(RuntimeHostError::fatal(
                "nemu_app_index_missing",
                "build_execution_backend_registry",
                RuntimeErrorCode::RuntimeFatal,
            ));
        }
        if self.entries.contains_key(&registration.instance_alias) {
            return Err(RuntimeHostError::fatal(
                "duplicate_instance_alias",
                "build_execution_backend_registry",
                RuntimeErrorCode::RuntimeFatal,
            ));
        }
        if self
            .entries
            .values()
            .any(|entry| entry.instance_id == registration.instance_id)
        {
            return Err(RuntimeHostError::fatal(
                "duplicate_instance_id",
                "build_execution_backend_registry",
                RuntimeErrorCode::RuntimeFatal,
            ));
        }
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
        let capabilities = Self::capability_profile(&registration.input, &registration.capture)?;
        let capabilities = match registration.provider_profile {
            Some(provider) => Self::merge_capability_profile(&capabilities, &provider)?,
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
        self.entries.insert(
            registration.instance_alias,
            ExecutionBackendEntry {
                instance_id: registration.instance_id,
                application_id: registration.application_id,
                application_adb,
                capabilities,
                nemu_app_index: registration.nemu_app_index,
                endpoint: Mutex::new(endpoint),
            },
        );
        Ok(())
    }

    pub fn with_vision_provider(
        mut self,
        vision_provider: Arc<dyn RecognitionVisionProvider>,
    ) -> Self {
        self.vision_provider = Some(vision_provider);
        self
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
}

impl fmt::Debug for ExecutionBackendRegistry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExecutionBackendRegistry")
            .field("instance_count", &self.entries.len())
            .field("vision_provider", &self.vision_provider.is_some())
            .finish()
    }
}

impl ExecutionBackendProvider for ExecutionBackendRegistry {
    fn instance_aliases(&self) -> Vec<String> {
        self.entries.keys().cloned().collect()
    }

    fn resolve(&self, instance_alias: &str) -> Option<ResolvedExecutionInstance> {
        let entry = self.entries.get(instance_alias)?;
        let (state, audit_endpoint, configuration) = {
            let endpoint = entry.endpoint();
            (
                endpoint.state.clone(),
                endpoint.audit_endpoint.clone(),
                endpoint.configuration.clone(),
            )
        };
        let resolved = ResolvedExecutionInstance::new(entry.instance_id, audit_endpoint)
            .with_configuration(configuration)
            .with_capabilities(entry.capabilities.clone());
        Some(match state {
            ResolvedInstanceEndpoint::Bound(adb_endpoint) => {
                resolved.with_adb_endpoint(adb_endpoint)
            }
            ResolvedInstanceEndpoint::Pending(pending) => resolved.with_pending_endpoint(pending),
        })
    }

    fn open_input(&self, instance_alias: &str) -> DeviceResult<Box<dyn InputBackend>> {
        let entry = self
            .entries
            .get(instance_alias)
            .ok_or_else(|| DeviceError::fatal("execution backend instance is not registered"))?;
        let input = {
            let endpoint = entry.endpoint();
            endpoint.require_bound("open_input")?;
            endpoint.input.clone()
        };
        create_touch_backend_for_fenced_input(input)
            .map(|backend| Box::new(backend) as Box<dyn InputBackend>)
    }

    fn open_capture(&self, instance_alias: &str) -> DeviceResult<Box<dyn CaptureBackend>> {
        let entry = self
            .entries
            .get(instance_alias)
            .ok_or_else(|| DeviceError::fatal("execution backend instance is not registered"))?;
        let capture = {
            let endpoint = entry.endpoint();
            endpoint.require_bound("open_capture")?;
            endpoint.capture.clone()
        };
        create_capture_backend(capture)
            .map(|selected| Box::new(selected) as Box<dyn CaptureBackend>)
    }

    fn open_nemu_session(&self, instance_alias: &str) -> DeviceResult<Option<NemuSessionBackends>> {
        let entry = self
            .entries
            .get(instance_alias)
            .ok_or_else(|| DeviceError::fatal("execution backend instance is not registered"))?;
        // Same pending guard as `open_input` / `open_capture`: a paired session is opened on
        // the bound target only.
        let (input, capture) = {
            let endpoint = entry.endpoint();
            if endpoint.input.requested != TouchBackendChoice::NemuIpc {
                return Ok(None);
            }
            endpoint.require_bound("open_nemu_session")?;
            (endpoint.input.clone(), endpoint.capture.clone())
        };
        let application = NemuApplicationTarget::new(
            &entry.application_id,
            entry
                .nemu_app_index
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
        .map(Some)
    }

    fn control_application(
        &self,
        instance_alias: &str,
        action: ApplicationLifecycleAction,
    ) -> DeviceResult<()> {
        let entry = self
            .entries
            .get(instance_alias)
            .ok_or_else(|| DeviceError::fatal("execution backend instance is not registered"))?;
        let application_target = {
            let endpoint = entry.endpoint();
            endpoint.require_bound("control_application")?;
            endpoint.application_target.clone()
        };
        let serial = application_target.resolved_serial();
        let adb = Adb::new(entry.application_adb.clone());
        adb.ensure_device(&serial, application_target.connect)?;
        match action {
            ApplicationLifecycleAction::Launch => {
                adb.launch_package(&serial, &entry.application_id)?;
            }
            ApplicationLifecycleAction::Stop => {
                adb.force_stop(&serial, &entry.application_id)?;
            }
            ApplicationLifecycleAction::Restart => {
                adb.force_stop(&serial, &entry.application_id)?;
                thread::sleep(Duration::from_millis(500));
                adb.launch_package(&serial, &entry.application_id)?;
            }
        }
        Ok(())
    }

    /// The ADB baseline probe: the bound endpoint's `ensure_device` with a connect attempt
    /// allowed (a freshly started emulator answers `adb connect` a few seconds after the
    /// vendor reports it running). No session is opened.
    fn probe_adb_baseline(&self, instance_alias: &str) -> DeviceResult<()> {
        let entry = self
            .entries
            .get(instance_alias)
            .ok_or_else(|| DeviceError::fatal("execution backend instance is not registered"))?;
        let application_target = {
            let endpoint = entry.endpoint();
            endpoint.require_bound("probe_adb_baseline")?;
            endpoint.application_target.clone()
        };
        Adb::new(entry.application_adb.clone())
            .ensure_device(&application_target.resolved_serial(), true)
            .map(|_| ())
    }

    fn probe_adb_baseline_until(
        &self,
        instance_alias: &str,
        deadline: std::time::Instant,
        stopped: &dyn Fn() -> bool,
    ) -> DeviceResult<()> {
        let entry = self
            .entries
            .get(instance_alias)
            .ok_or_else(|| DeviceError::fatal("execution backend instance is not registered"))?;
        let application_target = {
            let endpoint = entry.endpoint();
            endpoint.require_bound("probe_adb_baseline")?;
            endpoint.application_target.clone()
        };
        Adb::new(entry.application_adb.clone())
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
    fn observe_foreground_application(
        &self,
        instance_alias: &str,
    ) -> DeviceResult<ForegroundApplicationObservation> {
        let entry = self
            .entries
            .get(instance_alias)
            .ok_or_else(|| DeviceError::fatal("execution backend instance is not registered"))?;
        let application_target = {
            let endpoint = entry.endpoint();
            endpoint.require_bound("observe_foreground_application")?;
            endpoint.application_target.clone()
        };
        let serial = application_target.resolved_serial();
        let adb = Adb::new(entry.application_adb.clone());
        adb.ensure_device(&serial, application_target.connect)?;
        Ok(ForegroundApplicationObservation {
            foreground: adb.foreground_package(&serial)?,
            assigned: entry.application_id.clone(),
        })
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
        let entry = self.entries.get(instance_alias).ok_or_else(|| {
            refused(
                "execution backend instance is not registered",
                "emulator_control.unregistered",
            )
        })?;
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
        let entry = self.entries.get(instance_alias).ok_or_else(|| {
            refused(
                "execution backend instance is not registered",
                "emulator_control.unregistered",
            )
        })?;
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

    fn vision_provider(&self) -> Option<Arc<dyn RecognitionVisionProvider>> {
        self.vision_provider.as_ref().map(Arc::clone)
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
