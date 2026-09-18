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
    TouchBackendChoice, TouchBackendConfig, create_capture_backend,
    create_touch_backend_for_fenced_input, mumu_state_wait,
};
pub use actingcommand_execution_kernel::{
    DiscoveredInstanceBinding, EmulatorControlFailure, EmulatorControlOutcome,
    EmulatorControlResult, ExecutionBackendProvider, RecognitionVisionProvider,
    ResolvedAdbEndpoint, ResolvedExecutionInstance, VisionFfiProvider, VisionModelIdentity,
};
use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

pub struct ExecutionBackendRegistration {
    instance_alias: String,
    instance_id: InstanceId,
    application_id: String,
    input: TouchBackendConfig,
    capture: CaptureBackendConfig,
    configuration: actingcommand_contract::EffectiveDeviceConfiguration,
    discovered: Option<DiscoveredInstanceBinding>,
    provider_profile: Option<EmulatorCapabilityProfile>,
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
            provider_profile: None,
        })
    }

    /// Marks the registration as bound through MuMu instance discovery.
    pub fn with_discovered_binding(mut self, discovered: DiscoveredInstanceBinding) -> Self {
        self.discovered = Some(discovered);
        self
    }

    /// Attaches the provider capability profile admitted at startup; its provider-owned rows
    /// replace the registry placeholder rows when the registration is registered.
    pub fn with_capability_profile(mut self, profile: EmulatorCapabilityProfile) -> Self {
        self.provider_profile = Some(profile);
        self
    }
}

#[derive(Clone)]
struct ExecutionBackendEntry {
    instance_id: InstanceId,
    audit_endpoint: String,
    adb_endpoint: ResolvedAdbEndpoint,
    application_id: String,
    application_adb: AdbConfig,
    application_target: DeviceTarget,
    input: TouchBackendConfig,
    capture: CaptureBackendConfig,
    configuration: actingcommand_contract::EffectiveDeviceConfiguration,
    capabilities: EmulatorCapabilityProfile,
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
        let audit_endpoint = registration.input.target.resolved_serial();
        let mut adb_endpoint = ResolvedAdbEndpoint::new(
            registration.input.target.host.clone(),
            registration.input.target.port,
            registration.input.target.serial.is_some(),
        );
        if let Some(discovered) = registration.discovered {
            adb_endpoint = adb_endpoint.with_discovered_binding(discovered);
        }
        let application_adb = registration.input.adb_config.clone();
        let application_target = registration.input.target.clone();
        let capabilities = Self::capability_profile(&registration.input, &registration.capture)?;
        let capabilities = match registration.provider_profile {
            Some(provider) => Self::merge_capability_profile(&capabilities, &provider)?,
            None => capabilities,
        };
        self.entries.insert(
            registration.instance_alias,
            ExecutionBackendEntry {
                instance_id: registration.instance_id,
                audit_endpoint,
                adb_endpoint,
                application_id: registration.application_id,
                application_adb,
                application_target,
                input: registration.input,
                capture: registration.capture,
                configuration: registration.configuration,
                capabilities,
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
        Some(
            ResolvedExecutionInstance::new(entry.instance_id, &entry.audit_endpoint)
                .with_adb_endpoint(entry.adb_endpoint.clone())
                .with_configuration(entry.configuration.clone())
                .with_capabilities(entry.capabilities.clone()),
        )
    }

    fn open_input(&self, instance_alias: &str) -> DeviceResult<Box<dyn InputBackend>> {
        let entry = self
            .entries
            .get(instance_alias)
            .ok_or_else(|| DeviceError::fatal("execution backend instance is not registered"))?;
        create_touch_backend_for_fenced_input(entry.input.clone())
            .map(|backend| Box::new(backend) as Box<dyn InputBackend>)
    }

    fn open_capture(&self, instance_alias: &str) -> DeviceResult<Box<dyn CaptureBackend>> {
        let entry = self
            .entries
            .get(instance_alias)
            .ok_or_else(|| DeviceError::fatal("execution backend instance is not registered"))?;
        create_capture_backend(entry.capture.clone())
            .map(|selected| Box::new(selected) as Box<dyn CaptureBackend>)
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
        let serial = entry.application_target.resolved_serial();
        let adb = Adb::new(entry.application_adb.clone());
        adb.ensure_device(&serial, entry.application_target.connect)?;
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
        let Some(discovered) = entry.adb_endpoint.discovered_binding() else {
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
