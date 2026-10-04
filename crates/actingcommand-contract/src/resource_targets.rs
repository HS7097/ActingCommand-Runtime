// SPDX-License-Identifier: AGPL-3.0-only

//! Instance resource target policies (Workflow #308 RT-S1a): the receipt of the formal
//! `ApplyResourceTargets` entry and its field-positioned rejection. The document itself is
//! parsed and checked by the policy crate; the Runtime stores the checked policy as the
//! instance fact `session.resource_targets`.

use crate::{EventId, RuntimeContractError, RuntimeContractResult};
use serde::{Deserialize, Serialize};

/// Largest `ApplyResourceTargets` document, in UTF-8 bytes.
pub const MAX_RESOURCE_TARGETS_DOCUMENT_BYTES: usize = 64 * 1024;
/// Most targets one policy document declares.
pub const MAX_RESOURCE_TARGETS: usize = 16;
const MAX_REJECTION_FIELD_PATH_BYTES: usize = 4096;

/// The applied (or replayed) policy of one instance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceTargetsApplied {
    pub instance_alias: String,
    /// `sha256:<hex>` of the policy crate's canonical serialization of the document.
    pub policy_sha256: String,
    /// Sequence of the `fact.published` event that holds the policy.
    pub version: u64,
    pub event_id: EventId,
    /// Sequence of the policy active before this request; equals `version` on a replay.
    pub previous_version: Option<u64>,
    /// The same policy was already active and unexpired; nothing was appended.
    pub replayed: bool,
    /// Hash of the active catalog the document was checked against.
    pub checked_catalog_hash: String,
    /// Absent exactly when the policy withdraws every target (`targets: []`).
    pub valid_until_unix_ms: Option<u64>,
    /// Ledger position of the projection the conditions were computed from.
    pub conditions_at_position: u64,
    /// One entry per target, in document order: what the target currently observes.
    pub conditions: Vec<ResourceTargetCondition>,
}

/// One target's current observation, separate from the applied configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceTargetCondition {
    pub target_id: String,
    pub resource: String,
    pub fact_key: String,
    pub state: ResourceTargetConditionState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ResourceTargetConditionState {
    /// A valid inventory observation; `gap == 0` means the target is satisfied.
    Computed {
        current: i64,
        observed_at_unix_ms: u64,
        gap: u64,
    },
    /// No usable inventory observation yet; the policy is applied and waits for one.
    AwaitingObservation { reason: ResourceTargetPendingReason },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceTargetPendingReason {
    Missing,
    Expired,
    LowConfidence,
    InvalidValue,
}

/// Why and where a policy document was rejected; nothing was recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceTargetsRejection {
    /// JSON pointer of the offending field; empty for the whole document.
    pub field_path: String,
    pub line: u32,
    pub column: u32,
    pub reason: ResourceTargetsRejectionReason,
}

/// The closed set of document rejection reasons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceTargetsRejectionReason {
    InvalidJson,
    DuplicateKey,
    UnknownField,
    MissingField,
    InvalidType,
    InvalidValue,
    OutOfRange,
    UnsupportedSchemaVersion,
    DuplicateId,
    UnknownInstance,
    ValidityOutOfRange,
    CatalogUnavailable,
    UnknownResource,
    ResourceNotObservable,
    ResourceOutOfScope,
    UnknownTask,
    TaskOutOfScope,
    TaskDisabled,
    UnmappedTask,
    DuplicateTask,
}

/// What one instance can target and the resource target policy it holds now
/// (`ResourceTargetView`, Workflow #338 R5). `targetable` and `not_targetable` judge every pool
/// of the active catalog with the predicates of an `actingcommand.resource-targets.v2` check;
/// `active` is the stored policy as the evaluator reads it. Read-only; nothing is recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceTargetView {
    pub instance_alias: String,
    /// The value a policy document's `instance` field takes for this instance.
    pub policy_instance: String,
    pub evaluated_at_unix_ms: u64,
    /// Ledger position of the projection the view was computed from.
    pub as_of_ledger_position: u64,
    /// Hash of the active catalog; absent, with both lists empty, when none is active.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_hash: Option<String>,
    pub targetable: Vec<TargetableResource>,
    pub not_targetable: Vec<NotTargetableResource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<ActiveResourceTargets>,
}

/// A pool a v2 target of this instance may name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetableResource {
    pub resource: String,
    pub fact_key: String,
    /// The tasks of the instance whose run produces the pool, in catalog order.
    pub producing_tasks: Vec<String>,
    /// The pool valuation's `scale`; absent when a target must state its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_scale: Option<u64>,
    /// The pool valuation's `gap.weight_milli`; absent when a target must state its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_importance_milli: Option<u64>,
    pub scale_required: bool,
    pub importance_required: bool,
    pub observation: ResourceTargetObservation,
}

/// The pool's inventory as the time-validity projection shows it to the instance: a current
/// value with its observation time, or why there is none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceTargetObservation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_at_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending: Option<ResourceTargetPendingReason>,
}

/// A pool no target of this instance may name, with the reason a document naming it meets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotTargetableResource {
    pub resource: String,
    pub reason: ResourceTargetsRejectionReason,
}

/// The instance's stored policy: its identity, the `fact.published` event that holds it, its
/// targets in document form and, per target whose pool still resolves, what it observes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActiveResourceTargets {
    pub policy_sha256: String,
    pub schema_version: String,
    pub version: u64,
    pub event_id: EventId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_until_unix_ms: Option<u64>,
    pub expired: bool,
    pub targets: Vec<serde_json::Value>,
    pub conditions: Vec<ResourceTargetCondition>,
}

impl ResourceTargetView {
    pub fn validate(&self) -> RuntimeContractResult<()> {
        let invalid = || RuntimeContractError::new("invalid_resource_target_view");
        crate::validate_instance_alias(&self.instance_alias).map_err(|_| invalid())?;
        if self.policy_instance.is_empty()
            || self.evaluated_at_unix_ms == 0
            || self
                .catalog_hash
                .as_deref()
                .is_some_and(|hash| !canonical_sha256(hash))
            || (self.catalog_hash.is_none()
                && !(self.targetable.is_empty() && self.not_targetable.is_empty()))
            || self.targetable.iter().any(|resource| {
                resource.scale_required != resource.default_scale.is_none()
                    || resource.importance_required != resource.default_importance_milli.is_none()
                    || resource.observation.pending.is_some()
                        != resource.observation.current.is_none()
                    || resource.observation.current.is_some()
                        != resource.observation.observed_at_unix_ms.is_some()
            })
        {
            return Err(invalid());
        }
        if let Some(active) = &self.active
            && (!canonical_sha256(&active.policy_sha256)
                || active.version == 0
                || active.targets.is_empty()
                || active.targets.len() > MAX_RESOURCE_TARGETS
                || active.conditions.len() > active.targets.len())
        {
            return Err(invalid());
        }
        Ok(())
    }
}

impl ResourceTargetsApplied {
    pub fn validate(&self) -> RuntimeContractResult<()> {
        let invalid = || RuntimeContractError::new("invalid_resource_targets_applied");
        crate::validate_instance_alias(&self.instance_alias).map_err(|_| invalid())?;
        if !canonical_sha256(&self.policy_sha256)
            || !canonical_sha256(&self.checked_catalog_hash)
            || self.version == 0
            || self.conditions.len() > MAX_RESOURCE_TARGETS
            || self.conditions.is_empty() != self.valid_until_unix_ms.is_none()
        {
            return Err(invalid());
        }
        let history = if self.replayed {
            self.previous_version == Some(self.version)
        } else {
            self.previous_version
                .is_none_or(|previous| previous < self.version)
        };
        if !history {
            return Err(invalid());
        }
        Ok(())
    }
}

impl ResourceTargetsRejection {
    pub fn validate(&self) -> RuntimeContractResult<()> {
        if (!self.field_path.is_empty() && !self.field_path.starts_with('/'))
            || self.field_path.len() > MAX_REJECTION_FIELD_PATH_BYTES
            || self.field_path.chars().any(char::is_control)
            || self.line == 0
            || self.column == 0
        {
            return Err(RuntimeContractError::new(
                "invalid_resource_targets_rejection",
            ));
        }
        Ok(())
    }
}

fn canonical_sha256(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}
