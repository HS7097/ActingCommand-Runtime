// SPDX-License-Identifier: AGPL-3.0-only

//! Typed instance, server, and game facts projected from the GlobalLedger.

use crate::{EventId, EventType, ProjectedArtifactReference, SanitizationError};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

const MAX_FACT_KEY_BYTES: usize = 192;
const MAX_FACT_TEXT_BYTES: usize = 1_024;
const MAX_FACT_RECORDS: usize = 256;
const MAX_FACT_RECORD_FIELDS: usize = 64;
const MAX_INVALIDATION_EVENTS: usize = 32;
pub const MIN_FACT_TTL_MS: u64 = 1;
pub const MAX_FACT_TTL_MS: u64 = 31_536_000_000;

/// Selects the ownership and sharing boundary for a fact.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum FactScope {
    Instance { instance_id: String },
    Server { server_id: String },
    Game { game_id: String },
}

/// Scalar field value used inside a bounded record-list fact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum FactScalar {
    Boolean(bool),
    Integer(i64),
    String(String),
    TimestampMs(u64),
    DurationMs(u64),
}

/// Closed value model accepted by scheduling predicates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum FactValue {
    Boolean(bool),
    Integer(i64),
    String(String),
    TimestampMs(u64),
    DurationMs(u64),
    RecordList(Vec<BTreeMap<String, FactScalar>>),
}

/// Keeps small values inline while large evidence remains artifact-backed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum FactContent {
    Inline {
        value: FactValue,
    },
    Artifact {
        artifact: ProjectedArtifactReference,
    },
}

/// Identifies the reviewed authority that selected a fact family's TTL bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FactTtlSource {
    DetectorContract,
    CatalogPolicy,
    RuntimeDefault,
}

/// Pins the legal freshness interval for one published fact family.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FactTtlPolicy {
    pub minimum_ms: u64,
    pub maximum_ms: u64,
    pub source: FactTtlSource,
}

/// Durable fact publication record owned and validated by Runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FactRecord {
    pub scope: FactScope,
    pub key: String,
    pub content: FactContent,
    pub observed_at_unix_ms: u64,
    pub expires_at_unix_ms: Option<u64>,
    #[serde(default)]
    pub ttl_policy: Option<FactTtlPolicy>,
    pub confidence_milli: u16,
    pub source_detector: String,
    pub source_snapshot_id: String,
    pub schema_version: String,
    pub resource_bundle_hash: String,
    pub invalidate_on: Vec<EventType>,
}

/// Auditable reason and trigger for invalidating one published fact snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FactInvalidationEventData {
    pub scope: FactScope,
    pub key: String,
    pub source_snapshot_id: String,
    pub invalidated_at_unix_ms: u64,
    pub invalidated_by_event_id: EventId,
    pub invalidated_by_event_type: EventType,
}

/// Instance identity plus its server and game sharing scopes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstanceFactContext {
    pub instance_id: String,
    pub server_id: String,
    pub game_id: String,
}

/// Immutable fact projection pinned to one GlobalLedger position.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstanceFactSnapshot {
    pub snapshot_id: String,
    pub ledger_position: u64,
    pub context: InstanceFactContext,
    pub records: Vec<FactRecord>,
}

/// Explicit reasons why a fact cannot be consumed as known input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FactUnknownReason {
    Missing,
    Expired,
    LowConfidence,
    NonInline,
    TypeMismatch,
}

/// Resolution result that preserves unknown instead of coercing it to false.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FactResolution<'a> {
    Known(&'a FactRecord),
    Unknown {
        reason: FactUnknownReason,
        detection_key: String,
    },
}

impl FactRecord {
    /// Validates the bounded storage, freshness, provenance, and invalidation contract.
    pub fn validate(&self) -> Result<(), SanitizationError> {
        self.scope.validate()?;
        validate_fact_key(&self.key)?;
        validate_token(&self.source_detector, "source_detector")?;
        validate_token(&self.source_snapshot_id, "source_snapshot_id")?;
        validate_token(&self.schema_version, "schema_version")?;
        validate_sha256(&self.resource_bundle_hash, "resource_bundle_hash")?;
        if self.observed_at_unix_ms == 0
            || self.confidence_milli > 1_000
            || self.invalidate_on.len() > MAX_INVALIDATION_EVENTS
            || self
                .invalidate_on
                .iter()
                .enumerate()
                .any(|(index, event_type)| self.invalidate_on[..index].contains(event_type))
            || self.invalidate_on.iter().any(|event_type| {
                matches!(
                    event_type,
                    EventType::FactPublished | EventType::FactInvalidated
                )
            })
        {
            return Err(SanitizationError::new("invalid_fact_record", "fact"));
        }
        match (&self.expires_at_unix_ms, &self.ttl_policy) {
            (None, None) => {}
            (Some(expires), Some(policy)) => {
                policy.validate()?;
                let ttl_ms = expires
                    .checked_sub(self.observed_at_unix_ms)
                    .ok_or_else(|| {
                        SanitizationError::new("fact_ttl_expiry_invalid", "expires_at_unix_ms")
                    })?;
                if !(policy.minimum_ms..=policy.maximum_ms).contains(&ttl_ms) {
                    return Err(SanitizationError::new(
                        "fact_ttl_out_of_bounds",
                        "expires_at_unix_ms",
                    ));
                }
            }
            _ => {
                return Err(SanitizationError::new(
                    "fact_ttl_policy_missing",
                    "ttl_policy",
                ));
            }
        }
        self.content.validate()
    }

    /// Reports expiry without mutating or deleting the durable fact.
    pub fn is_expired(&self, now_unix_ms: u64) -> bool {
        self.expires_at_unix_ms
            .is_some_and(|expires| now_unix_ms > expires)
    }
}

impl FactTtlPolicy {
    pub fn validate(&self) -> Result<(), SanitizationError> {
        if self.minimum_ms < MIN_FACT_TTL_MS
            || self.maximum_ms > MAX_FACT_TTL_MS
            || self.minimum_ms > self.maximum_ms
        {
            return Err(SanitizationError::new(
                "fact_ttl_policy_invalid",
                "ttl_policy",
            ));
        }
        Ok(())
    }
}

impl FactScope {
    /// Validates the selected scope identifier.
    pub fn validate(&self) -> Result<(), SanitizationError> {
        let (value, field) = match self {
            Self::Instance { instance_id } => (instance_id, "instance_id"),
            Self::Server { server_id } => (server_id, "server_id"),
            Self::Game { game_id } => (game_id, "game_id"),
        };
        validate_token(value, field)
    }

    /// Returns whether this scope contributes to an instance projection.
    pub fn matches(&self, context: &InstanceFactContext) -> bool {
        match self {
            Self::Instance { instance_id } => instance_id == &context.instance_id,
            Self::Server { server_id } => server_id == &context.server_id,
            Self::Game { game_id } => game_id == &context.game_id,
        }
    }

    fn specificity(&self) -> u8 {
        match self {
            Self::Instance { .. } => 3,
            Self::Server { .. } => 2,
            Self::Game { .. } => 1,
        }
    }
}

impl FactContent {
    fn validate(&self) -> Result<(), SanitizationError> {
        match self {
            Self::Inline { value } => value.validate(),
            Self::Artifact { artifact } => artifact.validate(),
        }
    }
}

impl FactValue {
    fn validate(&self) -> Result<(), SanitizationError> {
        match self {
            Self::String(value) => validate_text(value, "fact_value"),
            Self::RecordList(records) => {
                if records.len() > MAX_FACT_RECORDS {
                    return Err(SanitizationError::new(
                        "fact_record_limit_exceeded",
                        "fact_value",
                    ));
                }
                for record in records {
                    if record.len() > MAX_FACT_RECORD_FIELDS {
                        return Err(SanitizationError::new(
                            "fact_field_limit_exceeded",
                            "fact_value",
                        ));
                    }
                    for (key, value) in record {
                        validate_token(key, "fact_field")?;
                        if let FactScalar::String(value) = value {
                            validate_text(value, "fact_field_value")?;
                        }
                    }
                }
                Ok(())
            }
            Self::Boolean(_) | Self::Integer(_) | Self::TimestampMs(_) | Self::DurationMs(_) => {
                Ok(())
            }
        }
    }
}

impl InstanceFactContext {
    /// Validates all three identifiers used to build a scoped projection.
    pub fn validate(&self) -> Result<(), SanitizationError> {
        validate_token(&self.instance_id, "instance_id")?;
        validate_token(&self.server_id, "server_id")?;
        validate_token(&self.game_id, "game_id")
    }
}

impl InstanceFactSnapshot {
    /// Validates the pinned projection and rejects duplicate or out-of-scope records.
    pub fn validate(&self) -> Result<(), SanitizationError> {
        validate_token(&self.snapshot_id, "fact_snapshot_id")?;
        self.context.validate()?;
        if self.ledger_position == 0 || self.records.len() > MAX_FACT_RECORDS {
            return Err(SanitizationError::new(
                "invalid_fact_snapshot",
                "fact_snapshot",
            ));
        }
        let mut identities = BTreeSet::new();
        for record in &self.records {
            record.validate()?;
            if !record.scope.matches(&self.context)
                || !identities.insert((record.scope.clone(), record.key.clone()))
            {
                return Err(SanitizationError::new(
                    "invalid_fact_snapshot_record",
                    "fact_snapshot",
                ));
            }
        }
        Ok(())
    }

    /// Resolves the most specific fact while preserving missing and stale states as unknown.
    pub fn resolve(&self, key: &str, now_unix_ms: u64) -> FactResolution<'_> {
        let candidate = self
            .records
            .iter()
            .filter(|record| record.key == key && record.scope.matches(&self.context))
            .max_by_key(|record| record.scope.specificity());
        let Some(record) = candidate else {
            return FactResolution::Unknown {
                reason: FactUnknownReason::Missing,
                detection_key: key.to_owned(),
            };
        };
        if record.is_expired(now_unix_ms) {
            return FactResolution::Unknown {
                reason: FactUnknownReason::Expired,
                detection_key: key.to_owned(),
            };
        }
        if record.confidence_milli == 0 {
            return FactResolution::Unknown {
                reason: FactUnknownReason::LowConfidence,
                detection_key: key.to_owned(),
            };
        }
        FactResolution::Known(record)
    }

    /// Resolves only path-safe inline strings from the `env.` fact family.
    pub fn resolve_environment_string(
        &self,
        key: &str,
        now_unix_ms: u64,
    ) -> Result<&str, FactUnknownReason> {
        let fact_key = format!("env.{key}");
        let record = match self.resolve(&fact_key, now_unix_ms) {
            FactResolution::Known(record) => record,
            FactResolution::Unknown { reason, .. } => return Err(reason),
        };
        let FactContent::Inline {
            value: FactValue::String(value),
        } = &record.content
        else {
            return Err(match &record.content {
                FactContent::Artifact { .. } => FactUnknownReason::NonInline,
                FactContent::Inline { .. } => FactUnknownReason::TypeMismatch,
            });
        };
        if !path_safe_string(value) {
            return Err(FactUnknownReason::TypeMismatch);
        }
        Ok(value)
    }
}

pub(crate) fn validate_fact_invalidation(
    data: &FactInvalidationEventData,
) -> Result<(), SanitizationError> {
    data.scope.validate()?;
    validate_fact_key(&data.key)?;
    validate_token(&data.source_snapshot_id, "source_snapshot_id")?;
    if data.invalidated_at_unix_ms == 0
        || matches!(
            data.invalidated_by_event_type,
            EventType::FactPublished | EventType::FactInvalidated
        )
    {
        return Err(SanitizationError::new(
            "invalid_fact_invalidation",
            "fact_invalidation",
        ));
    }
    Ok(())
}

fn validate_fact_key(value: &str) -> Result<(), SanitizationError> {
    const FAMILIES: [&str; 8] = [
        "identity.",
        "display.",
        "session.",
        "resource.",
        "inventory.",
        "timeline.",
        "health.",
        "env.",
    ];
    if value.len() > MAX_FACT_KEY_BYTES || !FAMILIES.iter().any(|prefix| value.starts_with(prefix))
    {
        return Err(SanitizationError::new("invalid_fact_key", "fact_key"));
    }
    validate_token(value, "fact_key")
}

fn validate_token(value: &str, field: &'static str) -> Result<(), SanitizationError> {
    if value.is_empty() || value.len() > MAX_FACT_TEXT_BYTES || value.chars().any(char::is_control)
    {
        return Err(SanitizationError::new("invalid_fact_token", field));
    }
    Ok(())
}

fn validate_text(value: &str, field: &'static str) -> Result<(), SanitizationError> {
    if value.len() > MAX_FACT_TEXT_BYTES || value.chars().any(char::is_control) {
        return Err(SanitizationError::new("invalid_fact_text", field));
    }
    Ok(())
}

fn validate_sha256(value: &str, field: &'static str) -> Result<(), SanitizationError> {
    if value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(SanitizationError::new("invalid_fact_hash", field))
    }
}

fn path_safe_string(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && !value.contains('/')
        && !value.contains('\\')
        && !value.contains(':')
        && !value.contains("..")
        && !Path::new(value).is_absolute()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(scope: FactScope, value: &str, expires: Option<u64>) -> FactRecord {
        FactRecord {
            scope,
            key: "env.theme".to_owned(),
            content: FactContent::Inline {
                value: FactValue::String(value.to_owned()),
            },
            observed_at_unix_ms: 1_000,
            expires_at_unix_ms: expires,
            ttl_policy: expires.map(|_| FactTtlPolicy {
                minimum_ms: 1,
                maximum_ms: 10_000,
                source: FactTtlSource::DetectorContract,
            }),
            confidence_milli: 900,
            source_detector: "detector.theme".to_owned(),
            source_snapshot_id: "snapshot:source".to_owned(),
            schema_version: "fact.v1".to_owned(),
            resource_bundle_hash: "a".repeat(64),
            invalidate_on: vec![EventType::RuntimeTakeover],
        }
    }

    #[test]
    fn scope_specificity_and_expiry_are_explicit() {
        let snapshot = InstanceFactSnapshot {
            snapshot_id: "snapshot:fact".to_owned(),
            ledger_position: 1,
            context: InstanceFactContext {
                instance_id: "instance-a".to_owned(),
                server_id: "server-a".to_owned(),
                game_id: "game-a".to_owned(),
            },
            records: vec![
                record(
                    FactScope::Server {
                        server_id: "server-a".to_owned(),
                    },
                    "Shared",
                    None,
                ),
                record(
                    FactScope::Instance {
                        instance_id: "instance-a".to_owned(),
                    },
                    "Specific",
                    Some(2_000),
                ),
            ],
        };
        snapshot.validate().expect("snapshot");
        assert_eq!(
            snapshot.resolve_environment_string("theme", 1_500),
            Ok("Specific")
        );
        assert_eq!(
            snapshot.resolve_environment_string("theme", 3_000),
            Err(FactUnknownReason::Expired)
        );
    }

    #[test]
    fn environment_strings_have_a_separate_path_safety_gate() {
        let snapshot = InstanceFactSnapshot {
            snapshot_id: "snapshot:fact".to_owned(),
            ledger_position: 1,
            context: InstanceFactContext {
                instance_id: "instance-a".to_owned(),
                server_id: "server-a".to_owned(),
                game_id: "game-a".to_owned(),
            },
            records: vec![record(
                FactScope::Instance {
                    instance_id: "instance-a".to_owned(),
                },
                "../unsafe",
                None,
            )],
        };
        assert_eq!(
            snapshot.resolve_environment_string("theme", 1_500),
            Err(FactUnknownReason::TypeMismatch)
        );
    }

    #[test]
    fn expiring_facts_require_a_bounded_ttl_policy() {
        let valid = record(
            FactScope::Instance {
                instance_id: "instance-a".to_owned(),
            },
            "Neutral",
            Some(2_000),
        );
        valid.validate().expect("bounded fact TTL");

        let mut missing = valid.clone();
        missing.ttl_policy = None;
        assert_eq!(
            missing
                .validate()
                .expect_err("missing TTL policy must be rejected")
                .code(),
            "fact_ttl_policy_missing"
        );

        let mut legacy_shape = serde_json::to_value(&valid).expect("fact JSON");
        legacy_shape
            .as_object_mut()
            .expect("fact object")
            .remove("ttl_policy");
        let decoded: FactRecord = serde_json::from_value(legacy_shape).expect("legacy fact shape");
        assert_eq!(
            decoded
                .validate()
                .expect_err("legacy expiring fact must be rejected explicitly")
                .code(),
            "fact_ttl_policy_missing"
        );

        let mut below_family_minimum = valid.clone();
        below_family_minimum.ttl_policy = Some(FactTtlPolicy {
            minimum_ms: 2_000,
            maximum_ms: 3_000,
            source: FactTtlSource::CatalogPolicy,
        });
        assert_eq!(
            below_family_minimum
                .validate()
                .expect_err("out-of-family TTL must be rejected")
                .code(),
            "fact_ttl_out_of_bounds"
        );

        let mut invalid_family = valid;
        invalid_family.ttl_policy = Some(FactTtlPolicy {
            minimum_ms: MAX_FACT_TTL_MS,
            maximum_ms: MIN_FACT_TTL_MS,
            source: FactTtlSource::RuntimeDefault,
        });
        assert_eq!(
            invalid_family
                .validate()
                .expect_err("invalid TTL policy must be rejected")
                .code(),
            "fact_ttl_policy_invalid"
        );

        let mut oversized_family = record(
            FactScope::Instance {
                instance_id: "instance-a".to_owned(),
            },
            "Neutral",
            Some(2_000),
        );
        oversized_family.ttl_policy = Some(FactTtlPolicy {
            minimum_ms: MIN_FACT_TTL_MS,
            maximum_ms: MAX_FACT_TTL_MS + 1,
            source: FactTtlSource::DetectorContract,
        });
        assert_eq!(
            oversized_family
                .validate()
                .expect_err("oversized TTL family must be rejected")
                .code(),
            "fact_ttl_policy_invalid"
        );
    }
}
