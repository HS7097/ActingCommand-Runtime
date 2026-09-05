// SPDX-License-Identifier: AGPL-3.0-only

use crate::{RuntimeHostError, RuntimeHostResult};
use actingcommand_contract::{
    ApplicationLifecycleAction, InstanceId, MAX_INSTANCE_ALIAS_BYTES, RuntimeErrorCode,
};
use actingcommand_device::{
    Adb, AdbConfig, CaptureBackend, CaptureBackendChoice, CaptureBackendConfig, DeviceError,
    DeviceResult, DeviceTarget, InputBackend, TouchBackendChoice, TouchBackendConfig,
    create_capture_backend, create_touch_backend,
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
        Ok(Self {
            instance_alias,
            instance_id,
            application_id,
            input,
            capture,
        })
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
                .with_configuration(actingcommand_contract::EffectiveDeviceConfiguration {
                    input_backend: entry.input.requested.as_str().to_owned(),
                    capture_backend: entry.capture.requested.as_str().to_owned(),
                    input_adb: entry.input.adb_config.adb_path.clone(),
                    capture_adb: entry.capture.adb_config.adb_path.clone(),
                    configured_serial: entry.input.target.serial.clone(),
                    resolved_serial: entry.input.target.resolved_serial(),
                    input_command_timeout_ms: entry.input.adb_config.command_timeout.as_millis(),
                    capture_command_timeout_ms: entry
                        .capture
                        .adb_config
                        .command_timeout
                        .as_millis(),
                    capture_timeout_ms: entry.capture.capture_timeout.as_millis(),
                    configured_mumu_root: entry.capture.nemu.nemu_folder.clone(),
                    configured_capture_dll: entry.capture.nemu.dll_path.clone(),
                }),
        )
    }

    fn open_input(&self, instance_alias: &str) -> DeviceResult<Box<dyn InputBackend>> {
        let entry = self
            .entries
            .get(instance_alias)
            .ok_or_else(|| DeviceError::fatal("execution backend instance is not registered"))?;
        create_touch_backend(entry.input.clone())
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

    fn vision_provider(&self) -> Option<Arc<dyn RecognitionVisionProvider>> {
        self.vision_provider.as_ref().map(Arc::clone)
    }
}

fn validate_alias(alias: &str) -> RuntimeHostResult<()> {
    if alias.is_empty()
        || alias.len() > MAX_INSTANCE_ALIAS_BYTES
        || alias.chars().any(char::is_control)
    {
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
