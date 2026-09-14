// SPDX-License-Identifier: AGPL-3.0-only

use crate::{RuntimeHostError, RuntimeHostResult};
use actingcommand_contract::{
    ApplicationLifecycleAction, InstanceId, MAX_INSTANCE_ALIAS_BYTES, RuntimeErrorCode,
};
use actingcommand_device::{
    Adb, AdbConfig, CaptureBackend, CaptureBackendChoice, CaptureBackendConfig, DeviceError,
    DeviceResult, DeviceTarget, InputBackend, NemuAppIndex, NemuApplicationTarget, NemuInputConfig,
    NemuIpcSession, NemuSessionBackends, TouchBackendChoice, TouchBackendConfig,
    create_capture_backend, create_touch_backend_for_fenced_input,
};
pub use actingcommand_execution_kernel::{
    ExecutionBackendProvider, RecognitionVisionProvider, ResolvedExecutionInstance,
    VisionFfiProvider, VisionModelIdentity,
};
use std::collections::{BTreeMap, BTreeSet};
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
            nemu_app_index: None,
        })
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

#[derive(Clone)]
struct ExecutionBackendEntry {
    instance_id: InstanceId,
    audit_endpoint: String,
    application_id: String,
    application_adb: AdbConfig,
    application_target: DeviceTarget,
    input: TouchBackendConfig,
    capture: CaptureBackendConfig,
    configuration: actingcommand_contract::EffectiveDeviceConfiguration,
    nemu_app_index: Option<NemuAppIndex>,
}

pub struct ExecutionBackendRegistry {
    entries: BTreeMap<String, ExecutionBackendEntry>,
    vision_provider: Option<Arc<dyn RecognitionVisionProvider>>,
}

impl ExecutionBackendRegistry {
    pub fn new(
        registrations: impl IntoIterator<Item = ExecutionBackendRegistration>,
    ) -> RuntimeHostResult<Self> {
        let mut entries = BTreeMap::new();
        let mut instance_ids = BTreeSet::new();
        for registration in registrations {
            if registration.input.requested == TouchBackendChoice::NemuIpc
                && registration.nemu_app_index.is_none()
            {
                return Err(RuntimeHostError::fatal(
                    "nemu_app_index_missing",
                    "build_execution_backend_registry",
                    RuntimeErrorCode::RuntimeFatal,
                ));
            }
            if entries.contains_key(&registration.instance_alias) {
                return Err(RuntimeHostError::fatal(
                    "duplicate_instance_alias",
                    "build_execution_backend_registry",
                    RuntimeErrorCode::RuntimeFatal,
                ));
            }
            if !instance_ids.insert(registration.instance_id) {
                return Err(RuntimeHostError::fatal(
                    "duplicate_instance_id",
                    "build_execution_backend_registry",
                    RuntimeErrorCode::RuntimeFatal,
                ));
            }
            let audit_endpoint = registration.input.target.resolved_serial();
            let application_adb = registration.input.adb_config.clone();
            let application_target = registration.input.target.clone();
            entries.insert(
                registration.instance_alias,
                ExecutionBackendEntry {
                    instance_id: registration.instance_id,
                    audit_endpoint,
                    application_id: registration.application_id,
                    application_adb,
                    application_target,
                    input: registration.input,
                    capture: registration.capture,
                    configuration: registration.configuration,
                    nemu_app_index: registration.nemu_app_index,
                },
            );
        }
        if entries.is_empty() {
            return Err(RuntimeHostError::fatal(
                "empty_execution_backend_registry",
                "build_execution_backend_registry",
                RuntimeErrorCode::RuntimeFatal,
            ));
        }
        Ok(Self {
            entries,
            vision_provider: None,
        })
    }

    pub fn with_vision_provider(
        mut self,
        vision_provider: Arc<dyn RecognitionVisionProvider>,
    ) -> Self {
        self.vision_provider = Some(vision_provider);
        self
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
                .with_configuration(entry.configuration.clone()),
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

    fn open_nemu_session(&self, instance_alias: &str) -> DeviceResult<Option<NemuSessionBackends>> {
        let entry = self
            .entries
            .get(instance_alias)
            .ok_or_else(|| DeviceError::fatal("execution backend instance is not registered"))?;
        if entry.input.requested != TouchBackendChoice::NemuIpc {
            return Ok(None);
        }
        let application = NemuApplicationTarget::new(
            &entry.application_id,
            entry
                .nemu_app_index
                .ok_or_else(|| DeviceError::fatal("Nemu application index is missing"))?,
        )?;
        NemuIpcSession::open(
            entry.capture.clone(),
            application,
            NemuInputConfig {
                command_timeout: entry.input.adb_config.command_timeout,
                shutdown_timeout: entry.input.maatouch_config.shutdown_timeout,
                tap_hold: entry.input.maatouch_config.tap_hold,
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
