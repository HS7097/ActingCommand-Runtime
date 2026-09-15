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
use crate::fact::FactValue;
use serde::{Deserialize, Serialize};

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
    /// Checks the schema identity, the record bound, and every record.
    pub fn validate(&self) -> Result<(), SanitizationError> {
        if self.schema_version != RUNTIME_FACT_SCHEMA_VERSION {
            return Err(SanitizationError::new(
                "runtime_fact_schema_version_mismatch",
                "schema_version",
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
            .try_for_each(RuntimeFactRecord::validate)
    }
}
