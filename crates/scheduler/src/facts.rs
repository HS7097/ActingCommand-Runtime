// SPDX-License-Identifier: AGPL-3.0-only

//! Memory-only store for the Runtime's own facts (Workflow #313).
//!
//! The store is pure: it reads no clock, touches no ledger, and performs no
//! I/O. The host owns the single instance, appends every accepted record to the
//! `GlobalLedger` before calling [`RuntimeFactStore::record`], seals the store
//! with [`RuntimeFactStore::snapshot`] on a fixed period, and rebuilds it after
//! a restart with [`RuntimeFactStore::replay`] followed by the records appended
//! after that snapshot. Anything not appended is gone with the process, by
//! design (iron rule 13).

use actingcommand_contract::{
    InstanceId, MAX_RUNTIME_FACTS, RUNTIME_FACT_SCHEMA_VERSION, RuntimeFactInvalidation,
    RuntimeFactInvalidationReason, RuntimeFactRecord, RuntimeFactScope, RuntimeFactSnapshot,
};
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;

/// Outcome of [`RuntimeFactStore::record`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeFactChange {
    /// No record existed for the key and scope.
    Inserted,
    /// A record existed and was replaced by a newer observation.
    Updated,
    /// The identical record was already present; nothing changed.
    Unchanged,
}

/// Rejections raised by the store. Every variant is a caller error or a bound;
/// none is silently absorbed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeFactError {
    /// The record failed contract validation.
    Invalid { code: &'static str },
    /// The store already holds a newer observation for the key and scope.
    Stale { existing_observed_at_unix_ms: u64 },
    /// [`MAX_RUNTIME_FACTS`] live records are already held.
    CapacityExceeded { limit: usize },
    /// No record exists for the key and scope.
    Missing,
}

impl fmt::Display for RuntimeFactError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid { code } => write!(formatter, "runtime fact rejected: {code}"),
            Self::Stale {
                existing_observed_at_unix_ms,
            } => write!(
                formatter,
                "runtime fact is older than the stored observation at {existing_observed_at_unix_ms}"
            ),
            Self::CapacityExceeded { limit } => {
                write!(formatter, "runtime fact store holds {limit} records")
            }
            Self::Missing => formatter.write_str("runtime fact is not present"),
        }
    }
}

impl Error for RuntimeFactError {}

/// Bounded map of live runtime facts keyed by scope and key.
#[derive(Debug, Default, Clone)]
pub struct RuntimeFactStore {
    active: BTreeMap<(RuntimeFactScope, String), RuntimeFactRecord>,
}

impl RuntimeFactStore {
    /// Empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Accepts one record. A strictly newer observation replaces the stored
    /// one; the identical record is idempotent; an observation at the same or
    /// an older millisecond is rejected, so a late writer can never roll a
    /// value back and two producers cannot race within one millisecond.
    pub fn record(
        &mut self,
        record: RuntimeFactRecord,
    ) -> Result<RuntimeFactChange, RuntimeFactError> {
        record
            .validate()
            .map_err(|error| RuntimeFactError::Invalid { code: error.code() })?;
        let key = (record.scope.clone(), record.key.clone());
        match self.active.get(&key) {
            Some(existing) if *existing == record => Ok(RuntimeFactChange::Unchanged),
            Some(existing) if existing.observed_at_unix_ms >= record.observed_at_unix_ms => {
                Err(RuntimeFactError::Stale {
                    existing_observed_at_unix_ms: existing.observed_at_unix_ms,
                })
            }
            Some(_) => {
                self.active.insert(key, record);
                Ok(RuntimeFactChange::Updated)
            }
            None => {
                if self.active.len() >= MAX_RUNTIME_FACTS {
                    return Err(RuntimeFactError::CapacityExceeded {
                        limit: MAX_RUNTIME_FACTS,
                    });
                }
                self.active.insert(key, record);
                Ok(RuntimeFactChange::Inserted)
            }
        }
    }

    /// Drops one record and returns the audit entry the host appends.
    pub fn invalidate(
        &mut self,
        scope: &RuntimeFactScope,
        key: &str,
        reason: RuntimeFactInvalidationReason,
        at_unix_ms: u64,
    ) -> Result<RuntimeFactInvalidation, RuntimeFactError> {
        let removed = self
            .active
            .remove(&(scope.clone(), key.to_owned()))
            .ok_or(RuntimeFactError::Missing)?;
        Ok(RuntimeFactInvalidation {
            scope: removed.scope,
            key: removed.key,
            reason,
            at_unix_ms,
        })
    }

    /// Drops every record of one instance whose key starts with one of the
    /// given family prefixes, in key order. Used on owner takeover and device
    /// close.
    pub fn invalidate_instance(
        &mut self,
        instance_id: InstanceId,
        families: &[&str],
        reason: RuntimeFactInvalidationReason,
        at_unix_ms: u64,
    ) -> Vec<RuntimeFactInvalidation> {
        let scope = RuntimeFactScope::Instance { instance_id };
        let keys = self
            .active
            .range((scope.clone(), String::new())..)
            .take_while(|((candidate, _), _)| *candidate == scope)
            .filter(|((_, key), _)| families.iter().any(|family| key.starts_with(family)))
            .map(|((_, key), _)| key.clone())
            .collect::<Vec<_>>();
        keys.into_iter()
            .map(|key| {
                self.active.remove(&(scope.clone(), key.clone()));
                RuntimeFactInvalidation {
                    scope: scope.clone(),
                    key,
                    reason,
                    at_unix_ms,
                }
            })
            .collect()
    }

    /// Current record for a key and scope, if any.
    pub fn get(&self, scope: &RuntimeFactScope, key: &str) -> Option<&RuntimeFactRecord> {
        self.active.get(&(scope.clone(), key.to_owned()))
    }

    /// All live records in scope and key order.
    pub fn records(&self) -> impl Iterator<Item = &RuntimeFactRecord> {
        self.active.values()
    }

    /// Number of live records.
    pub fn len(&self) -> usize {
        self.active.len()
    }

    /// Whether no record is held.
    pub fn is_empty(&self) -> bool {
        self.active.is_empty()
    }

    /// Sealed image of every live record, bound to the ledger position the host
    /// observed when sealing. Expired records are included; expiry is a read
    /// predicate evaluated by consumers.
    pub fn snapshot(&self, ledger_position: u64, taken_at_unix_ms: u64) -> RuntimeFactSnapshot {
        RuntimeFactSnapshot {
            schema_version: RUNTIME_FACT_SCHEMA_VERSION.to_owned(),
            ledger_position,
            taken_at_unix_ms,
            records: self.active.values().cloned().collect(),
        }
    }

    /// Replaces the whole store with a sealed image.
    pub fn replay(&mut self, snapshot: &RuntimeFactSnapshot) -> Result<usize, RuntimeFactError> {
        snapshot
            .validate()
            .map_err(|error| RuntimeFactError::Invalid { code: error.code() })?;
        self.active.clear();
        for record in &snapshot.records {
            self.active
                .insert((record.scope.clone(), record.key.clone()), record.clone());
        }
        Ok(self.active.len())
    }
}
