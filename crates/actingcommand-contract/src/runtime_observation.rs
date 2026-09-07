// SPDX-License-Identifier: AGPL-3.0-only

//! Typed state facts committed by the existing Runtime and monitor owners.

use crate::{
    EventId, OwnerEpoch, RuntimeControlPlaneStatus, RuntimeMonitorInstanceStatus,
    RuntimeMonitorRegistryStatus, RuntimeMonitorState, SanitizationError,
};
use serde::{Deserialize, Serialize};

pub const MAX_RUNTIME_OBSERVED_INSTANCES: usize = 1_024;
pub const MAX_MONITOR_IMPORT_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeStateSource {
    pub event_id: EventId,
    pub sequence: u64,
    pub sampled_started_at_unix_ms: u64,
    pub sampled_completed_at_unix_ms: u64,
}

impl RuntimeStateSource {
    pub fn validate(&self) -> Result<(), SanitizationError> {
        if self.sequence == 0 {
            return Err(invalid("sequence"));
        }
        validate_interval(
            self.sampled_started_at_unix_ms,
            self.sampled_completed_at_unix_ms,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RuntimeObservedState {
    ControlPlane {
        status: RuntimeControlPlaneStatus,
    },
    Monitor {
        status: RuntimeMonitorRegistryStatus,
    },
    ProjectCurrent {
        status: RuntimeControlPlaneStatus,
        fatal: bool,
    },
}

impl RuntimeObservedState {
    pub fn owner_epoch(&self) -> OwnerEpoch {
        match self {
            Self::ControlPlane { status } | Self::ProjectCurrent { status, .. } => {
                status.owner_epoch()
            }
            Self::Monitor { status } => status.owner_epoch(),
        }
    }

    fn validate(&self) -> Result<(), SanitizationError> {
        let count = match self {
            Self::ControlPlane { status } | Self::ProjectCurrent { status, .. } => {
                status.validate().map_err(|_| invalid("control_plane"))?;
                if status.source().is_some() {
                    return Err(invalid("sample_source"));
                }
                status.instances().len()
            }
            Self::Monitor { status } => {
                status.validate().map_err(|_| invalid("monitor"))?;
                if status.source().is_some() {
                    return Err(invalid("sample_source"));
                }
                status.instances().len()
            }
        };
        if count > MAX_RUNTIME_OBSERVED_INSTANCES {
            return Err(invalid("instances"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeMonitorJournalSource {
    pub sha256: String,
    pub length: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeMonitorImport {
    pub source: Option<RuntimeMonitorJournalSource>,
    pub revision: u64,
    pub instances: Vec<RuntimeMonitorInstanceStatus>,
}

impl RuntimeMonitorImport {
    pub fn validate(&self) -> Result<(), SanitizationError> {
        if let Some(source) = &self.source {
            if source.length > MAX_MONITOR_IMPORT_BYTES
                || source.sha256.len() != 64
                || !source
                    .sha256
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
                || (source.length == 0 && (self.revision != 0 || !self.instances.is_empty()))
                || (source.length != 0 && self.revision == 0)
            {
                return Err(invalid("monitor_import_source"));
            }
        } else if self.revision != 0 || !self.instances.is_empty() {
            return Err(invalid("monitor_import_missing_source"));
        }
        if self.instances.len() > MAX_RUNTIME_OBSERVED_INSTANCES {
            return Err(invalid("monitor_import_instances"));
        }
        let mut previous = None;
        for instance in &self.instances {
            instance
                .validate()
                .map_err(|_| invalid("monitor_import_status"))?;
            if instance.policy().is_none()
                || previous.is_some_and(|alias| alias >= instance.instance_alias())
            {
                return Err(invalid("monitor_import_order"));
            }
            previous = Some(instance.instance_alias());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeMonitorChangeKind {
    Configure,
    Clear,
    Probe,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeMonitorChange {
    pub kind: RuntimeMonitorChangeKind,
    pub previous_revision: u64,
    pub revision: u64,
    pub configuration_version: u64,
    pub applied: bool,
    pub status: RuntimeMonitorInstanceStatus,
    pub probe_configuration_version: Option<u64>,
    pub probe_state: Option<RuntimeMonitorState>,
}

impl RuntimeMonitorChange {
    pub fn validate(&self) -> Result<(), SanitizationError> {
        self.status
            .validate()
            .map_err(|_| invalid("monitor_status"))?;
        let expected_revision = self
            .previous_revision
            .checked_add(u64::from(self.applied))
            .ok_or_else(|| invalid("monitor_revision"))?;
        if self.revision != expected_revision || self.configuration_version > self.revision {
            return Err(invalid("monitor_revision"));
        }
        match self.kind {
            RuntimeMonitorChangeKind::Configure | RuntimeMonitorChangeKind::Clear => {
                if self.probe_configuration_version.is_some()
                    || self.probe_state.is_some()
                    || (self.applied && self.configuration_version != self.revision)
                    || (self.kind == RuntimeMonitorChangeKind::Clear
                        && self.status.policy().is_some())
                    || (self.kind == RuntimeMonitorChangeKind::Configure
                        && self.status.policy().is_none())
                {
                    return Err(invalid("monitor_configuration"));
                }
            }
            RuntimeMonitorChangeKind::Probe => {
                let version = self
                    .probe_configuration_version
                    .ok_or_else(|| invalid("monitor_probe_version"))?;
                let state = self
                    .probe_state
                    .as_ref()
                    .ok_or_else(|| invalid("monitor_probe_state"))?;
                state
                    .validate()
                    .map_err(|_| invalid("monitor_probe_state"))?;
                if version == 0
                    || version > self.configuration_version
                    || (self.applied
                        && (version != self.configuration_version
                            || self.status.state() != Some(state)))
                {
                    return Err(invalid("monitor_probe_version"));
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RuntimeStateFact {
    Observed {
        sampled_started_at_unix_ms: u64,
        sampled_completed_at_unix_ms: u64,
        state: RuntimeObservedState,
    },
    MonitorImported {
        owner_epoch: OwnerEpoch,
        import: RuntimeMonitorImport,
    },
    MonitorChanged {
        owner_epoch: OwnerEpoch,
        change: Box<RuntimeMonitorChange>,
    },
}

impl RuntimeStateFact {
    pub fn validate(&self) -> Result<(), SanitizationError> {
        match self {
            Self::Observed {
                sampled_started_at_unix_ms,
                sampled_completed_at_unix_ms,
                state,
            } => {
                validate_interval(*sampled_started_at_unix_ms, *sampled_completed_at_unix_ms)?;
                state.validate()
            }
            Self::MonitorImported { import, .. } => import.validate(),
            Self::MonitorChanged { change, .. } => change.validate(),
        }
    }
}

fn validate_interval(started: u64, completed: u64) -> Result<(), SanitizationError> {
    if started == 0 || completed < started {
        return Err(invalid("sample_interval"));
    }
    Ok(())
}

fn invalid(field: &'static str) -> SanitizationError {
    SanitizationError::new("invalid_runtime_state_fact", field)
}
