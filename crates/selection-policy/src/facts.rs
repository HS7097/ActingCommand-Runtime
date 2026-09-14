// SPDX-License-Identifier: AGPL-3.0-only

//! The explicit inputs an evaluation reads: the candidate set and the fact snapshot.
//!
//! Both are passed in whole. The evaluator never goes looking for more, so both are part of
//! the hashed input and the same pair always produces the same decision.
//!
//! A fact that is absent, past its expiry, past the freshness bound its document declares,
//! below the confidence its document requires, or not a scalar resolves to a typed
//! [`UnknownReason`]. It never resolves to `false`, to `0`, or to an empty string.

use std::collections::BTreeMap;

use actingcommand_contract::{
    FactContent, FactRecord, FactScope, FactValue, InstanceFactSnapshot as ContractFactSnapshot,
};
use serde::{Deserialize, Serialize};

use crate::schema::{FactDeclaration, ValueType};

/// Closed scalar model shared by candidate fields and facts.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ScalarValue {
    Integer(i64),
    Boolean(bool),
    String(String),
}

impl ScalarValue {
    pub(crate) fn matches(&self, value_type: &ValueType) -> bool {
        match (self, value_type) {
            (Self::Integer(_), ValueType::Integer) => true,
            (Self::Boolean(_), ValueType::Boolean) => true,
            (Self::String(value), ValueType::EnumString { allowed }) => {
                allowed.iter().any(|member| member == value)
            }
            _ => false,
        }
    }
}

/// One candidate projected from one screen, addressed by its projection identifier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Candidate {
    pub candidate_id: String,
    pub fields: BTreeMap<String, ScalarValue>,
}

/// Why an input could not be read as a known value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnknownReason {
    /// The snapshot carries no record under this fact key.
    FactMissing,
    /// The record's own expiry has passed.
    FactExpired,
    /// The record is older than the freshness bound the document declares.
    FactStale,
    /// The record's confidence is below the floor the document declares.
    FactLowConfidence,
    /// The record holds an artifact or a record list, not a scalar.
    FactNotScalar,
    /// The candidate carries no value under this declared field name.
    FieldMissing,
    /// The value present does not have the declared type.
    TypeMismatch,
    /// No lookup entry covers this value and the transform declares no default.
    LookupMiss,
}

/// One usable fact, or a recorded reason why the published record is not usable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum SelectionFactEntry {
    Published {
        value: ScalarValue,
        published_at_unix_ms: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expires_at_unix_ms: Option<u64>,
        confidence_milli: u16,
    },
    Unusable {
        reason: UnknownReason,
    },
}

/// The instance fact projection an evaluation is allowed to read, pinned to one snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectionFactSnapshot {
    pub snapshot_id: String,
    pub snapshot_at_unix_ms: u64,
    pub facts: BTreeMap<String, SelectionFactEntry>,
}

impl SelectionFactSnapshot {
    /// Builds a snapshot from published fact records, keeping the most specific scope per key.
    ///
    /// A record whose content is an artifact or a record list is kept as
    /// [`UnknownReason::FactNotScalar`] rather than dropped, so the decision can say why the
    /// fact was not usable instead of reporting it as absent.
    pub fn from_fact_records(
        snapshot_id: impl Into<String>,
        snapshot_at_unix_ms: u64,
        records: &[FactRecord],
    ) -> Self {
        let mut chosen: BTreeMap<String, (u8, SelectionFactEntry)> = BTreeMap::new();
        for record in records {
            let specificity = scope_specificity(&record.scope);
            let entry = entry_for(record);
            match chosen.get(&record.key) {
                Some((previous, _)) if *previous >= specificity => {}
                _ => {
                    chosen.insert(record.key.clone(), (specificity, entry));
                }
            }
        }
        Self {
            snapshot_id: snapshot_id.into(),
            snapshot_at_unix_ms,
            facts: chosen
                .into_iter()
                .map(|(key, (_, entry))| (key, entry))
                .collect(),
        }
    }

    /// Builds a snapshot from a contract fact projection, keeping only records in its context.
    pub fn from_instance_snapshot(
        snapshot: &ContractFactSnapshot,
        snapshot_at_unix_ms: u64,
    ) -> Self {
        let records: Vec<FactRecord> = snapshot
            .records
            .iter()
            .filter(|record| record.scope.matches(&snapshot.context))
            .cloned()
            .collect();
        Self::from_fact_records(&snapshot.snapshot_id, snapshot_at_unix_ms, &records)
    }

    pub(crate) fn resolve(
        &self,
        declaration: &FactDeclaration,
        now_unix_ms: u64,
    ) -> Result<&ScalarValue, UnknownReason> {
        let entry = self
            .facts
            .get(&declaration.fact_key)
            .ok_or(UnknownReason::FactMissing)?;
        let (value, published_at_unix_ms, expires_at_unix_ms, confidence_milli) = match entry {
            SelectionFactEntry::Unusable { reason } => return Err(*reason),
            SelectionFactEntry::Published {
                value,
                published_at_unix_ms,
                expires_at_unix_ms,
                confidence_milli,
            } => (
                value,
                *published_at_unix_ms,
                *expires_at_unix_ms,
                *confidence_milli,
            ),
        };
        if expires_at_unix_ms.is_some_and(|expiry| now_unix_ms >= expiry) {
            return Err(UnknownReason::FactExpired);
        }
        if now_unix_ms.saturating_sub(published_at_unix_ms) > declaration.max_age_ms {
            return Err(UnknownReason::FactStale);
        }
        if confidence_milli < declaration.minimum_confidence_milli || confidence_milli == 0 {
            return Err(UnknownReason::FactLowConfidence);
        }
        if !value.matches(&declaration.value_type) {
            return Err(UnknownReason::TypeMismatch);
        }
        Ok(value)
    }
}

fn scope_specificity(scope: &FactScope) -> u8 {
    match scope {
        FactScope::Instance { .. } => 3,
        FactScope::Server { .. } => 2,
        FactScope::Game { .. } => 1,
    }
}

fn entry_for(record: &FactRecord) -> SelectionFactEntry {
    let FactContent::Inline { value } = &record.content else {
        return SelectionFactEntry::Unusable {
            reason: UnknownReason::FactNotScalar,
        };
    };
    let scalar = match value {
        FactValue::Boolean(value) => ScalarValue::Boolean(*value),
        FactValue::Integer(value) => ScalarValue::Integer(*value),
        FactValue::String(value) => ScalarValue::String(value.clone()),
        FactValue::TimestampMs(value) | FactValue::DurationMs(value) => {
            match i64::try_from(*value) {
                Ok(value) => ScalarValue::Integer(value),
                Err(_) => {
                    return SelectionFactEntry::Unusable {
                        reason: UnknownReason::TypeMismatch,
                    };
                }
            }
        }
        FactValue::RecordList(_) => {
            return SelectionFactEntry::Unusable {
                reason: UnknownReason::FactNotScalar,
            };
        }
    };
    SelectionFactEntry::Published {
        value: scalar,
        published_at_unix_ms: record.observed_at_unix_ms,
        expires_at_unix_ms: record.expires_at_unix_ms,
        confidence_milli: record.confidence_milli,
    }
}
