// SPDX-License-Identifier: AGPL-3.0-only

//! Program-side facts owned by the Runtime itself (Workflow #313).
//!
//! A runtime fact describes the Runtime's own state: device sessions and
//! self-checks, backend capabilities, task settlements, host pressure, lease
//! occupancy mirrors, the in-memory runtime configuration manifest, provider
//! readiness, and task eligibility. Observations of the game or application
//! under automation stay in [`crate::FactRecord`]; the two key families are
//! disjoint so a record can never be filed in the wrong store.
//!
//! The store that holds these records lives in `actingcommand-scheduler` and is
//! memory-only. Durability comes solely from the ledger: every accepted record
//! is appended as an event before it enters memory, and the whole store is
//! sealed periodically as a [`RuntimeFactSnapshot`] (iron rule 13).

use crate::event::{InstanceId, OriginModule, SanitizationError};
use crate::fact::{FactScalar, FactValue};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Schema identity carried by every sealed snapshot.
pub const RUNTIME_FACT_SCHEMA_VERSION: &str = "actingcommand.runtime-fact.v1";
/// Upper bound on live records held by one Runtime process.
pub const MAX_RUNTIME_FACTS: usize = 4_096;
/// Upper bound on one key, in bytes.
pub const MAX_RUNTIME_FACT_KEY_BYTES: usize = 128;
/// Upper bound on rows in a record-list value.
pub const MAX_RUNTIME_FACT_RECORD_ROWS: usize = 256;
/// Upper bound on fields per record-list row.
pub const MAX_RUNTIME_FACT_RECORD_FIELDS: usize = 64;
/// Upper bound on the serialized snapshot carried by one `runtime.fact_snapshot`
/// event: the same single-event payload bound the ledger applies to a
/// `fact.observed` event. A larger store is rejected, never truncated.
pub const MAX_RUNTIME_FACT_SNAPSHOT_BYTES: usize = crate::fact::MAX_FACT_OBSERVATION_BYTES;
/// Shortest period between two periodic `runtime.fact_snapshot` events.
pub const RUNTIME_FACT_SNAPSHOT_INTERVAL_MS: u64 = 60_000;
/// Key of the runtime fact that carries the manifest's subsystem rows.
pub const CONFIG_SUBSYSTEMS_FACT_KEY: &str = "config.subsystems";
/// Key of the runtime fact that carries the manifest's parameter rows.
pub const CONFIG_PARAMETERS_FACT_KEY: &str = "config.parameters";
/// Upper bound on subsystems in one configuration manifest.
pub const MAX_CONFIG_MANIFEST_SUBSYSTEMS: usize = 64;
/// Upper bound on parameters in one configuration manifest.
pub const MAX_CONFIG_MANIFEST_PARAMETERS: usize = 256;
/// Upper bound on a subsystem name or a parameter key, in bytes.
pub const MAX_CONFIG_MANIFEST_NAME_BYTES: usize = 128;
/// Upper bound on a subsystem reason, in bytes.
pub const MAX_CONFIG_MANIFEST_REASON_BYTES: usize = 512;
/// Key families accepted for runtime facts. None of them overlaps the
/// instance-fact families validated by `crate::fact`.
pub const RUNTIME_FACT_FAMILIES: [&str; 8] = [
    "device.",
    "backend.",
    "task.",
    "host.",
    "lease.",
    "config.",
    "provider.",
    "eligible.",
];

/// Validates a runtime fact key: bounded, printable, no whitespace, and inside
/// one of [`RUNTIME_FACT_FAMILIES`].
pub fn validate_runtime_fact_key(key: &str) -> Result<(), SanitizationError> {
    if key.is_empty()
        || key.len() > MAX_RUNTIME_FACT_KEY_BYTES
        || key
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        return Err(SanitizationError::new("invalid_runtime_fact_key", "key"));
    }
    if !RUNTIME_FACT_FAMILIES
        .iter()
        .any(|family| key.starts_with(family))
    {
        return Err(SanitizationError::new("runtime_fact_family_unknown", "key"));
    }
    Ok(())
}

/// Where a runtime fact applies.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RuntimeFactScope {
    /// The whole Runtime process.
    Runtime,
    /// One registered instance.
    Instance { instance_id: InstanceId },
}

/// One typed observation the Runtime made about itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeFactRecord {
    pub scope: RuntimeFactScope,
    pub key: String,
    pub value: FactValue,
    /// Wall-clock time the producing module observed the value.
    pub observed_at_unix_ms: u64,
    /// Module that produced the value; the host is the only writer.
    pub source: OriginModule,
    /// Optional lifetime; `None` means the record stays until superseded or
    /// invalidated.
    pub ttl_ms: Option<u64>,
}

impl RuntimeFactRecord {
    /// Checks key, value bounds, and lifetime. A record that fails here never
    /// reaches the ledger or the store.
    pub fn validate(&self) -> Result<(), SanitizationError> {
        validate_runtime_fact_key(&self.key)?;
        if let FactValue::RecordList(records) = &self.value
            && (records.len() > MAX_RUNTIME_FACT_RECORD_ROWS
                || records
                    .iter()
                    .any(|record| record.len() > MAX_RUNTIME_FACT_RECORD_FIELDS))
        {
            return Err(SanitizationError::new(
                "runtime_fact_record_list_too_large",
                "value",
            ));
        }
        if self.ttl_ms == Some(0) {
            return Err(SanitizationError::new("invalid_runtime_fact_ttl", "ttl_ms"));
        }
        Ok(())
    }

    /// Absolute expiry, when a lifetime was declared.
    pub fn expires_at_unix_ms(&self) -> Option<u64> {
        self.ttl_ms
            .and_then(|ttl| self.observed_at_unix_ms.checked_add(ttl))
    }

    /// Whether the record is past its declared lifetime at `now_unix_ms`.
    pub fn is_expired(&self, now_unix_ms: u64) -> bool {
        self.expires_at_unix_ms()
            .is_some_and(|expires_at| now_unix_ms >= expires_at)
    }
}

/// The in-memory runtime configuration manifest: which subsystems the host
/// runs and why, and the effective value of every parameter it applies, each
/// marked with where the value came from. It is data only; the host records
/// it as the two `config.*` runtime facts once per startup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeConfigManifest {
    pub subsystems: Vec<ConfigSubsystem>,
    pub parameters: Vec<ConfigParameter>,
}

/// One subsystem the host either runs or leaves out, with a short reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigSubsystem {
    pub name: String,
    pub enabled: bool,
    pub reason: String,
}

/// One effective parameter value and where it came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigParameter {
    pub key: String,
    pub value: FactScalar,
    pub source: ConfigParameterSource,
}

/// Where an effective parameter value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigParameterSource {
    /// Set in the configuration file.
    Explicit,
    /// A library default; nothing in the configuration file named it.
    Default,
    /// Learned at startup from the environment (for example discovery).
    Discovered,
}

impl ConfigParameterSource {
    /// The wire spelling, also used as the `source` row field.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Explicit => "explicit",
            Self::Default => "default",
            Self::Discovered => "discovered",
        }
    }
}

impl RuntimeConfigManifest {
    /// Checks the two list bounds and every name, key, and reason: non-empty,
    /// control-free, and within the byte bounds. A manifest that passes here
    /// always yields two records that pass [`RuntimeFactRecord::validate`].
    pub fn validate(&self) -> Result<(), SanitizationError> {
        if self.subsystems.len() > MAX_CONFIG_MANIFEST_SUBSYSTEMS {
            return Err(SanitizationError::new(
                "config_manifest_subsystems_too_many",
                "subsystems",
            ));
        }
        if self.parameters.len() > MAX_CONFIG_MANIFEST_PARAMETERS {
            return Err(SanitizationError::new(
                "config_manifest_parameters_too_many",
                "parameters",
            ));
        }
        for subsystem in &self.subsystems {
            if !manifest_text_ok(&subsystem.name, MAX_CONFIG_MANIFEST_NAME_BYTES) {
                return Err(SanitizationError::new(
                    "invalid_config_subsystem_name",
                    "subsystems",
                ));
            }
            if !manifest_text_ok(&subsystem.reason, MAX_CONFIG_MANIFEST_REASON_BYTES) {
                return Err(SanitizationError::new(
                    "invalid_config_subsystem_reason",
                    "subsystems",
                ));
            }
        }
        for parameter in &self.parameters {
            if !manifest_text_ok(&parameter.key, MAX_CONFIG_MANIFEST_NAME_BYTES) {
                return Err(SanitizationError::new(
                    "invalid_config_parameter_key",
                    "parameters",
                ));
            }
        }
        Ok(())
    }

    /// Encodes the manifest as its two runtime facts, both in scope `runtime`
    /// with no lifetime and the same observation time: `config.subsystems`
    /// holds one row `{name, enabled, reason}` per subsystem and
    /// `config.parameters` one row `{key, value, source}` per parameter, in
    /// manifest order. Pure: nothing is sampled or appended here.
    pub fn to_fact_records(
        &self,
        observed_at_unix_ms: u64,
        source: OriginModule,
    ) -> [RuntimeFactRecord; 2] {
        let subsystems = self
            .subsystems
            .iter()
            .map(|subsystem| {
                BTreeMap::from([
                    (
                        "name".to_owned(),
                        FactScalar::String(subsystem.name.clone()),
                    ),
                    ("enabled".to_owned(), FactScalar::Boolean(subsystem.enabled)),
                    (
                        "reason".to_owned(),
                        FactScalar::String(subsystem.reason.clone()),
                    ),
                ])
            })
            .collect();
        let parameters = self
            .parameters
            .iter()
            .map(|parameter| {
                BTreeMap::from([
                    ("key".to_owned(), FactScalar::String(parameter.key.clone())),
                    ("value".to_owned(), parameter.value.clone()),
                    (
                        "source".to_owned(),
                        FactScalar::String(parameter.source.as_str().to_owned()),
                    ),
                ])
            })
            .collect();
        [
            RuntimeFactRecord {
                scope: RuntimeFactScope::Runtime,
                key: CONFIG_SUBSYSTEMS_FACT_KEY.to_owned(),
                value: FactValue::RecordList(subsystems),
                observed_at_unix_ms,
                source,
                ttl_ms: None,
            },
            RuntimeFactRecord {
                scope: RuntimeFactScope::Runtime,
                key: CONFIG_PARAMETERS_FACT_KEY.to_owned(),
                value: FactValue::RecordList(parameters),
                observed_at_unix_ms,
                source,
                ttl_ms: None,
            },
        ]
    }
}

/// Non-empty, control-free, and at most `max_bytes` long.
fn manifest_text_ok(value: &str, max_bytes: usize) -> bool {
    !value.is_empty() && value.len() <= max_bytes && !value.chars().any(char::is_control)
}

/// Why a runtime fact was dropped from the store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeFactInvalidationReason {
    /// A new owner epoch took over; device-bound facts are stale until the
    /// post-connect self-check writes them again.
    RuntimeTakeover,
    /// The instance's device session was closed.
    DeviceClosed,
    /// The declared lifetime elapsed.
    Expired,
    /// An operator asked for the value to be dropped.
    Operator,
}

/// Audit record for one dropped runtime fact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeFactInvalidation {
    pub scope: RuntimeFactScope,
    pub key: String,
    pub reason: RuntimeFactInvalidationReason,
    pub at_unix_ms: u64,
}

impl RuntimeFactInvalidation {
    /// Checks the dropped key; the reason and time are closed types.
    pub fn validate(&self) -> Result<(), SanitizationError> {
        validate_runtime_fact_key(&self.key)
    }
}

/// Sealed image of the whole store at one ledger position.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeFactSnapshot {
    pub schema_version: String,
    pub ledger_position: u64,
    pub taken_at_unix_ms: u64,
    pub records: Vec<RuntimeFactRecord>,
}

impl RuntimeFactSnapshot {
    /// Checks the schema identity, the ledger position, the record bound,
    /// every record, and the serialized size one ledger event may carry.
    pub fn validate(&self) -> Result<(), SanitizationError> {
        if self.schema_version != RUNTIME_FACT_SCHEMA_VERSION {
            return Err(SanitizationError::new(
                "runtime_fact_schema_version_mismatch",
                "schema_version",
            ));
        }
        if self.ledger_position == 0 {
            return Err(SanitizationError::new(
                "runtime_fact_ledger_position_invalid",
                "ledger_position",
            ));
        }
        if self.records.len() > MAX_RUNTIME_FACTS {
            return Err(SanitizationError::new(
                "runtime_fact_snapshot_too_large",
                "records",
            ));
        }
        self.records
            .iter()
            .try_for_each(RuntimeFactRecord::validate)?;
        if serde_json::to_vec(self)
            .map_err(|_| SanitizationError::new("invalid_runtime_fact_snapshot", "records"))?
            .len()
            > MAX_RUNTIME_FACT_SNAPSHOT_BYTES
        {
            return Err(SanitizationError::new(
                "runtime_fact_snapshot_payload_too_large",
                "records",
            ));
        }
        Ok(())
    }
}
