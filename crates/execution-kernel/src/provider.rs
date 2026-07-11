// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_contract::InstanceId;
use actingcommand_device::{CaptureBackend, DeviceResult, InputBackend};
use std::fmt;

#[derive(Clone, PartialEq, Eq)]
pub struct ResolvedExecutionInstance {
    instance_id: InstanceId,
    audit_endpoint: String,
}

impl ResolvedExecutionInstance {
    pub fn new(instance_id: InstanceId, audit_endpoint: impl Into<String>) -> Self {
        Self {
            instance_id,
            audit_endpoint: audit_endpoint.into(),
        }
    }

    pub const fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    pub fn audit_endpoint(&self) -> &str {
        &self.audit_endpoint
    }
}

impl fmt::Debug for ResolvedExecutionInstance {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResolvedExecutionInstance")
            .field("instance_id", &self.instance_id)
            .field("audit_endpoint", &"<redacted>")
            .finish()
    }
}

/// Daemon-only factory boundary. Implementations open backends inside execution worker threads.
pub trait ExecutionBackendProvider: Send + Sync + 'static {
    fn resolve(&self, instance_alias: &str) -> Option<ResolvedExecutionInstance>;

    fn open_input(&self, instance_alias: &str) -> DeviceResult<Box<dyn InputBackend>>;

    fn open_capture(&self, instance_alias: &str) -> DeviceResult<Box<dyn CaptureBackend>>;
}
