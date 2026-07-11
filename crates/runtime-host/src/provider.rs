// SPDX-License-Identifier: AGPL-3.0-only

use crate::{RuntimeHostError, RuntimeHostResult};
use actingcommand_contract::{InstanceId, MAX_INSTANCE_ALIAS_BYTES, RuntimeErrorCode};
use actingcommand_device::{
    CaptureBackend, DeviceError, DeviceResult, InputBackend, TouchBackendConfig,
    create_touch_backend,
};
use actingcommand_execution_kernel::{ExecutionBackendProvider, ResolvedExecutionInstance};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;

pub struct ResolvedInputInstance {
    instance_id: InstanceId,
    audit_endpoint: String,
}

impl ResolvedInputInstance {
    pub fn new(instance_id: InstanceId, audit_endpoint: impl Into<String>) -> Self {
        Self {
            instance_id,
            audit_endpoint: audit_endpoint.into(),
        }
    }

    pub const fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    pub(crate) fn audit_endpoint(&self) -> &str {
        &self.audit_endpoint
    }
}

impl fmt::Debug for ResolvedInputInstance {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResolvedInputInstance")
            .field("instance_id", &self.instance_id)
            .field("audit_endpoint", &"<redacted>")
            .finish()
    }
}

/// Immutable daemon-side adapter boundary for resolving and opening production input backends.
pub trait InputBackendProvider: Send + Sync + 'static {
    fn resolve(&self, instance_alias: &str) -> Option<ResolvedInputInstance>;

    fn open(&self, instance_alias: &str) -> DeviceResult<Box<dyn InputBackend>>;
}

/// C3b Task 3 adapter that moves input ownership into the execution kernel while capture remains
/// behind the C3a compatibility surface. Task 4 replaces this with the complete device registry.
pub(crate) struct InputOnlyExecutionProvider {
    input: Arc<dyn InputBackendProvider>,
}

impl InputOnlyExecutionProvider {
    pub(crate) fn new(input: Arc<dyn InputBackendProvider>) -> Self {
        Self { input }
    }
}

impl ExecutionBackendProvider for InputOnlyExecutionProvider {
    fn resolve(&self, instance_alias: &str) -> Option<ResolvedExecutionInstance> {
        self.input.resolve(instance_alias).map(|resolved| {
            ResolvedExecutionInstance::new(resolved.instance_id(), resolved.audit_endpoint())
        })
    }

    fn open_input(&self, instance_alias: &str) -> DeviceResult<Box<dyn InputBackend>> {
        self.input.open(instance_alias)
    }

    fn open_capture(&self, _instance_alias: &str) -> DeviceResult<Box<dyn CaptureBackend>> {
        Err(DeviceError::fatal(
            "daemon capture backend is not configured before C3b Task 4",
        ))
    }
}

pub struct TouchBackendRegistration {
    instance_alias: String,
    instance_id: InstanceId,
    config: TouchBackendConfig,
}

impl TouchBackendRegistration {
    pub fn new(
        instance_alias: impl Into<String>,
        instance_id: InstanceId,
        config: TouchBackendConfig,
    ) -> RuntimeHostResult<Self> {
        let instance_alias = instance_alias.into();
        validate_alias(&instance_alias)?;
        Ok(Self {
            instance_alias,
            instance_id,
            config,
        })
    }
}

#[derive(Clone)]
struct TouchBackendEntry {
    instance_id: InstanceId,
    config: TouchBackendConfig,
}

pub struct TouchBackendRegistry {
    entries: BTreeMap<String, TouchBackendEntry>,
}

impl TouchBackendRegistry {
    pub fn new(
        registrations: impl IntoIterator<Item = TouchBackendRegistration>,
    ) -> RuntimeHostResult<Self> {
        let mut entries = BTreeMap::new();
        let mut instance_ids = BTreeSet::new();
        for registration in registrations {
            if entries.contains_key(&registration.instance_alias) {
                return Err(RuntimeHostError::fatal(
                    "duplicate_instance_alias",
                    "build_touch_backend_registry",
                    RuntimeErrorCode::RuntimeFatal,
                ));
            }
            if !instance_ids.insert(registration.instance_id) {
                return Err(RuntimeHostError::fatal(
                    "duplicate_instance_id",
                    "build_touch_backend_registry",
                    RuntimeErrorCode::RuntimeFatal,
                ));
            }
            entries.insert(
                registration.instance_alias,
                TouchBackendEntry {
                    instance_id: registration.instance_id,
                    config: registration.config,
                },
            );
        }
        if entries.is_empty() {
            return Err(RuntimeHostError::fatal(
                "empty_input_backend_registry",
                "build_touch_backend_registry",
                RuntimeErrorCode::RuntimeFatal,
            ));
        }
        Ok(Self { entries })
    }
}

impl fmt::Debug for TouchBackendRegistry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TouchBackendRegistry")
            .field("instance_count", &self.entries.len())
            .finish()
    }
}

impl InputBackendProvider for TouchBackendRegistry {
    fn resolve(&self, instance_alias: &str) -> Option<ResolvedInputInstance> {
        let entry = self.entries.get(instance_alias)?;
        Some(ResolvedInputInstance::new(
            entry.instance_id,
            entry.config.target.resolved_serial(),
        ))
    }

    fn open(&self, instance_alias: &str) -> DeviceResult<Box<dyn InputBackend>> {
        let entry = self.entries.get(instance_alias).ok_or_else(|| {
            actingcommand_device::DeviceError::fatal("input backend instance is not registered")
        })?;
        create_touch_backend(entry.config.clone())
            .map(|backend| Box::new(backend) as Box<dyn InputBackend>)
    }
}

fn validate_alias(alias: &str) -> RuntimeHostResult<()> {
    if alias.is_empty()
        || alias.len() > MAX_INSTANCE_ALIAS_BYTES
        || alias.chars().any(char::is_control)
    {
        return Err(RuntimeHostError::fatal(
            "invalid_instance_alias",
            "build_touch_backend_registry",
            RuntimeErrorCode::RuntimeFatal,
        ));
    }
    Ok(())
}
