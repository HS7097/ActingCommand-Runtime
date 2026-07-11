// SPDX-License-Identifier: AGPL-3.0-only

use crate::{RuntimeHostError, RuntimeHostResult};
use actingcommand_contract::{InstanceId, MAX_INSTANCE_ALIAS_BYTES, RuntimeErrorCode};
use actingcommand_device::{
    CaptureBackend, CaptureBackendChoice, CaptureBackendConfig, DeviceError, DeviceResult,
    InputBackend, TouchBackendChoice, TouchBackendConfig, create_capture_backend,
    create_touch_backend,
};
pub use actingcommand_execution_kernel::{ExecutionBackendProvider, ResolvedExecutionInstance};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

pub struct ExecutionBackendRegistration {
    instance_alias: String,
    instance_id: InstanceId,
    input: TouchBackendConfig,
    capture: CaptureBackendConfig,
}

impl ExecutionBackendRegistration {
    pub fn new(
        instance_alias: impl Into<String>,
        instance_id: InstanceId,
        input: TouchBackendConfig,
        capture: CaptureBackendConfig,
    ) -> RuntimeHostResult<Self> {
        let instance_alias = instance_alias.into();
        validate_alias(&instance_alias)?;
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
            input,
            capture,
        })
    }
}

#[derive(Clone)]
struct ExecutionBackendEntry {
    instance_id: InstanceId,
    audit_endpoint: String,
    input: TouchBackendConfig,
    capture: CaptureBackendConfig,
}

pub struct ExecutionBackendRegistry {
    entries: BTreeMap<String, ExecutionBackendEntry>,
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
            entries.insert(
                registration.instance_alias,
                ExecutionBackendEntry {
                    instance_id: registration.instance_id,
                    audit_endpoint,
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
        Ok(Self { entries })
    }
}

impl fmt::Debug for ExecutionBackendRegistry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExecutionBackendRegistry")
            .field("instance_count", &self.entries.len())
            .finish()
    }
}

impl ExecutionBackendProvider for ExecutionBackendRegistry {
    fn instance_aliases(&self) -> Vec<String> {
        self.entries.keys().cloned().collect()
    }

    fn resolve(&self, instance_alias: &str) -> Option<ResolvedExecutionInstance> {
        let entry = self.entries.get(instance_alias)?;
        Some(ResolvedExecutionInstance::new(
            entry.instance_id,
            &entry.audit_endpoint,
        ))
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
        create_capture_backend(entry.capture.clone()).map(|selected| selected.backend)
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
