// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #455 G-R2a (`contracts/resource-targets.md`, "Document
//! `actingcommand.resource-targets.v3`"): the goal document's value store. A v3 document holds v2
//! targets, each with its own optional lifetime, and the values of named entries. The Runtime
//! stores it in the same instance fact `session.resource_targets`, without expiry, each row a
//! request wrote naming when and through which channel it was set. The evaluator reads a v3
//! record's targets as a v2 policy's: the targets whose own lifetime has not passed weigh as an
//! active v2 policy's, the others say what an expired v2 policy says. Nothing reads the entries
//! yet. Nothing here reads a clock, a file or the network.

use std::collections::{BTreeMap, BTreeSet};

use actingcommand_contract::{
    FactScalar as ContractFactScalar, Initiator, MAX_RESOURCE_TARGETS, RESOURCE_TARGETS_FACT_KEY,
    ResourceTargetCondition, ResourceTargetConditionState, ResourceTargetsRejection,
    ResourceTargetsRejectionReason, RuntimeChannel,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    CheckedResourceTargets, DecodedResourceTargetsV2, MAX_RESOURCE_TARGET_AMOUNT,
    MAX_RESOURCE_TARGET_MILLI, MAX_RESOURCE_TARGET_TASKS, MAX_RESOURCE_TARGETS_VALIDITY_MS,
    MAX_TASKS_PER_RESOURCE_TARGET, ParsedResourceTargets, RESOURCE_TARGETS_SCHEMA_VERSION_V2,
    ROW_KIND, ROW_POLICY, ROW_TARGET, ROW_TASK, ResourceTargetSpecV2, ResourceTargetsDocument,
    ResourceTargetsDocumentV2, ResourceTargetsError, RowsDecodeError, TargetApplicationV2,
    TargetCondition, TargetMode, TargetObservation, TargetPolicyState, TargetRule, check_target_v2,
    declaration_rejection, decode_rows_v2, encode_rows_v2, manual_offset_name, mode_name,
    observe_target, present, rejected_at, rule_name, valid_identifier, weight_name,
};
use crate::canonical::canonical_serialized;
use crate::codes::PolicyCode;
use crate::evaluator::project_time_validity;
use crate::source::{CatalogDocumentSource, parse_document};
use crate::{
    CompiledCatalog, EvaluationFacts, EvaluationTime, FactScalar, FactValue, InstanceSnapshot,
    ObservedFact, PoolSpec, SchedulingDocumentKind, ScopeSelector,
};

/// The goal document (Workflow #455 G-R2a): v2 targets with their own lifetimes, and entries.
pub const RESOURCE_TARGETS_SCHEMA_VERSION_V3: &str = "actingcommand.resource-targets.v3";
/// Most entries one v3 document or record holds: with 1 header, 16 targets and 128 task rows a
/// record holds at most 241 rows, inside the 256 the scheduler's input takes.
pub const MAX_RESOURCE_TARGET_ENTRIES: usize = 96;
/// Longest canonical text of an entry's value: the fact string bound.
pub const MAX_RESOURCE_TARGET_ENTRY_VALUE_BYTES: usize = 1_024;
/// Longest entry name.
const MAX_ENTRY_NAME_BYTES: usize = 64;

const ROW_ENTRY: &str = "entry";
const FIELD_VALID_UNTIL: &str = "valid_until_unix_ms";
const FIELD_SET_AT: &str = "set_unix_ms";
const FIELD_CHANNEL: &str = "channel";
const FIELD_INITIATOR: &str = "initiator";
const FIELD_NAME: &str = "name";
const FIELD_VALUE_JSON: &str = "value_json";

/// The `actingcommand.resource-targets.v3` document of one instance. It has no lifetime of its
/// own: each target may carry one. `entries`, when present, replaces every stored entry; absent,
/// the stored entries are kept.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceTargetsDocumentV3 {
    pub schema_version: String,
    pub instance: String,
    pub targets: Vec<ResourceTargetSpecV3>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub entries: Option<Vec<ResourceTargetEntry>>,
    /// A document-level `valid_until_unix_ms`, which a v3 document does not have: read only so
    /// that its refusal (`unknown_field`) names the member itself; never written.
    #[serde(
        rename = "valid_until_unix_ms",
        default,
        skip_serializing,
        deserialize_with = "present"
    )]
    document_lifetime: Option<serde_json::Value>,
}

/// A v2 target with its own lifetime, and a deadline that no Runtime takes before G-R4.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceTargetSpecV3 {
    pub id: String,
    pub resource: String,
    pub condition: TargetCondition,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub scale: Option<u64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub importance_milli: Option<u64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub rule: Option<TargetRule>,
    pub apply: TargetApplicationV2,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub tasks: Option<Vec<String>>,
    /// The target's own lifetime: absent, it does not expire; present, after now and at most one
    /// year ahead, as a v2 document's.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub valid_until_unix_ms: Option<u64>,
    /// A deadline (G-R4): in the shape, refused `deadline_unsupported` until G-R4.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub by_unix_ms: Option<u64>,
}

impl ResourceTargetSpecV3 {
    /// The target as a v2 target: every member but its lifetime and its deadline.
    pub fn target_v2(&self) -> ResourceTargetSpecV2 {
        ResourceTargetSpecV2 {
            id: self.id.clone(),
            resource: self.resource.clone(),
            condition: self.condition,
            scale: self.scale,
            importance_milli: self.importance_milli,
            rule: self.rule,
            apply: self.apply,
            tasks: self.tasks.clone(),
        }
    }

    /// The v2 `target` with `valid_until_unix_ms` as its own lifetime and no deadline.
    pub fn of_v2(target: &ResourceTargetSpecV2, valid_until_unix_ms: Option<u64>) -> Self {
        Self {
            id: target.id.clone(),
            resource: target.resource.clone(),
            condition: target.condition,
            scale: target.scale,
            importance_milli: target.importance_milli,
            rule: target.rule,
            apply: target.apply,
            tasks: target.tasks.clone(),
            valid_until_unix_ms,
            by_unix_ms: None,
        }
    }
}

/// One entry of a v3 document: a name (`[a-z0-9][a-z0-9_]*`, 1 to 64 bytes, unique in the
/// document) and any JSON value whose canonical text is at most 1024 bytes. This slice checks
/// the form only; the pack's declaration is not read here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceTargetEntry {
    pub name: String,
    pub value: serde_json::Value,
}

/// Who wrote the rows a request writes, and when: the Runtime's clock at the request and the
/// channel of the request's connection with the initiator it declared, never the document's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TargetSetter {
    pub set_unix_ms: u64,
    pub channel: RuntimeChannel,
    pub initiator: Option<Initiator>,
}

/// The record one `ApplyResourceTargets` writes over the instance's stored record, and what its
/// receipt says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposedResourceTargets {
    /// `sha256:<hex>` of the canonical serialization of the document the record holds.
    pub policy_sha256: String,
    pub rows: Vec<BTreeMap<String, ContractFactScalar>>,
    /// The record's expiry: a v1 or v2 policy's lifetime; none for a v3 record or a withdrawal.
    pub expires_at_unix_ms: Option<u64>,
    /// The receipt's `valid_until_unix_ms`: a v1 or v2 policy's lifetime; for a v3 record the
    /// latest lifetime of its targets when every target has one, else absent.
    pub valid_until_unix_ms: Option<u64>,
    /// The receipt's `conditions`: one per target, empty exactly when `valid_until_unix_ms` is.
    pub conditions: Vec<ResourceTargetCondition>,
}

/// The document a check passed, as composing its record needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum CheckedDocument {
    /// A v1 document, with the refusal it meets over a v3 record (at `/schema_version`).
    V1(Box<ResourceTargetsRejection>),
    V2(Box<ResourceTargetsDocumentV2>),
    V3(Box<ResourceTargetsDocumentV3>),
}

/// A v3 record's target whose own lifetime has passed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LapsedTarget {
    pub(crate) target: ResourceTargetSpecV2,
    pub(crate) valid_until_unix_ms: u64,
}

/// The version probe of the v3 document: whether its top-level `schema_version` is exactly the
/// v3 string. It refuses nothing.
pub(crate) fn declares_v3(bytes: &[u8]) -> bool {
    serde_json::from_slice::<serde_json::Value>(bytes).is_ok_and(|document| {
        document
            .get("schema_version")
            .and_then(serde_json::Value::as_str)
            == Some(RESOURCE_TARGETS_SCHEMA_VERSION_V3)
    })
}

/// The v3 half of `parse_resource_targets`: the declaration parse (unknown members, a
/// document-level `valid_until_unix_ms` among them, are `unknown_field`), then the schema
/// version, bounds and forms (16 targets, each a v2 target's, no `by_unix_ms`; 96 entries, each
/// name of its form and each value's canonical text), uniqueness (target ids, tasks, resources,
/// entry names) and the v2 apply rules.
pub(super) fn parse_resource_targets_v3(
    source: &CatalogDocumentSource,
) -> Result<ParsedResourceTargets, ResourceTargetsError> {
    use ResourceTargetsRejectionReason as Reason;
    let parsed = parse_document::<ResourceTargetsDocumentV3>(
        source,
        SchedulingDocumentKind::ResourceTargets,
    )
    .map_err(|diagnostic| declaration_rejection(*diagnostic))?;
    let (document, source_map) = (parsed.value, parsed.source_map);
    let at = |path: String, reason| rejected_at(&source_map, path, reason);
    if document.schema_version != RESOURCE_TARGETS_SCHEMA_VERSION_V3 {
        return Err(at(
            "/schema_version".to_owned(),
            Reason::UnsupportedSchemaVersion,
        ));
    }
    if document.document_lifetime.is_some() {
        return Err(at(format!("/{FIELD_VALID_UNTIL}"), Reason::UnknownField));
    }
    if document.targets.len() > MAX_RESOURCE_TARGETS {
        return Err(at("/targets".to_owned(), Reason::OutOfRange));
    }
    let mut task_references = 0_usize;
    for (index, target) in document.targets.iter().enumerate() {
        let path = format!("/targets/{index}");
        if !valid_identifier(&target.id) {
            return Err(at(format!("{path}/id"), Reason::InvalidValue));
        }
        let TargetCondition::AtLeast { amount } = target.condition;
        if !(1..=MAX_RESOURCE_TARGET_AMOUNT).contains(&amount) {
            return Err(at(format!("{path}/condition/amount"), Reason::OutOfRange));
        }
        if target
            .scale
            .is_some_and(|scale| !(1..=MAX_RESOURCE_TARGET_AMOUNT).contains(&scale))
        {
            return Err(at(format!("{path}/scale"), Reason::OutOfRange));
        }
        if target
            .importance_milli
            .is_some_and(|importance| !(1..=MAX_RESOURCE_TARGET_MILLI).contains(&importance))
        {
            return Err(at(format!("{path}/importance_milli"), Reason::OutOfRange));
        }
        if let Some(tasks) = &target.tasks {
            if !(1..=MAX_TASKS_PER_RESOURCE_TARGET).contains(&tasks.len()) {
                return Err(at(format!("{path}/tasks"), Reason::OutOfRange));
            }
            for task_index in 0..tasks.len() {
                task_references += 1;
                if task_references > MAX_RESOURCE_TARGET_TASKS {
                    return Err(at(format!("{path}/tasks/{task_index}"), Reason::OutOfRange));
                }
            }
        }
        if target.by_unix_ms.is_some() {
            return Err(at(
                format!("{path}/by_unix_ms"),
                Reason::DeadlineUnsupported,
            ));
        }
    }
    let entries = document.entries.as_deref().unwrap_or_default();
    if entries.len() > MAX_RESOURCE_TARGET_ENTRIES {
        return Err(at("/entries".to_owned(), Reason::OutOfRange));
    }
    for (index, entry) in entries.iter().enumerate() {
        if !valid_entry_name(&entry.name) {
            return Err(at(format!("/entries/{index}/name"), Reason::InvalidValue));
        }
        entry_value_text(&entry.value)
            .map_err(|reason| at(format!("/entries/{index}/value"), reason))?;
    }
    let mut target_ids = BTreeSet::new();
    let mut task_ids = BTreeSet::new();
    let mut resources = BTreeSet::new();
    for (index, target) in document.targets.iter().enumerate() {
        if !target_ids.insert(target.id.as_str()) {
            return Err(at(format!("/targets/{index}/id"), Reason::DuplicateId));
        }
        for (task_index, task) in target.tasks.iter().flatten().enumerate() {
            if !task_ids.insert(task.as_str()) {
                return Err(at(
                    format!("/targets/{index}/tasks/{task_index}"),
                    Reason::DuplicateTask,
                ));
            }
        }
        if !resources.insert(target.resource.as_str()) {
            return Err(at(
                format!("/targets/{index}/resource"),
                Reason::DuplicateId,
            ));
        }
    }
    let mut names = BTreeSet::new();
    for (index, entry) in entries.iter().enumerate() {
        if !names.insert(entry.name.as_str()) {
            return Err(at(format!("/entries/{index}/name"), Reason::DuplicateId));
        }
    }
    for (index, target) in document.targets.iter().enumerate() {
        match (target.apply.mode, target.apply.manual_offset, &target.tasks) {
            (TargetMode::Adjust, Some(_), _) => {
                return Err(at(
                    format!("/targets/{index}/apply/manual_offset"),
                    Reason::InvalidValue,
                ));
            }
            (TargetMode::Override, _, None) => {
                return Err(at(format!("/targets/{index}/tasks"), Reason::MissingField));
            }
            _ => {}
        }
    }
    Ok(ParsedResourceTargets {
        document: ResourceTargetsDocument {
            schema_version: document.schema_version.clone(),
            instance: document.instance.clone(),
            valid_until_unix_ms: None,
            targets: Vec::new(),
        },
        document_v2: None,
        document_v3: Some(document),
        source_map,
    })
}

/// The v3 half of `check_resource_targets`: the instance, each target's own lifetime in document
/// order, then per target its resource, whether its step and importance resolve, and its tasks,
/// each as a v2 target's. The record is composed later, over the stored one
/// ([`CheckedResourceTargets::compose`]): `rows` stays empty and `policy_sha256` names the
/// document as composed over no record.
pub(super) fn check_resource_targets_v3(
    parsed: &ParsedResourceTargets,
    document: &ResourceTargetsDocumentV3,
    catalog: &CompiledCatalog,
    facts: &EvaluationFacts,
    time: EvaluationTime,
) -> Result<CheckedResourceTargets, ResourceTargetsError> {
    use ResourceTargetsRejectionReason as Reason;
    let at = |path: String, reason| rejected_at(&parsed.source_map, path, reason);
    let instance = facts
        .instances
        .iter()
        .find(|instance| instance.instance_id == document.instance)
        .ok_or_else(|| at("/instance".to_owned(), Reason::UnknownInstance))?;
    let latest = time
        .unix_ms
        .saturating_add(MAX_RESOURCE_TARGETS_VALIDITY_MS);
    for (index, target) in document.targets.iter().enumerate() {
        if let Some(valid_until) = target.valid_until_unix_ms
            && (valid_until <= time.unix_ms || valid_until > latest)
        {
            return Err(at(
                format!("/targets/{index}/{FIELD_VALID_UNTIL}"),
                Reason::ValidityOutOfRange,
            ));
        }
    }
    let mut resolved = Vec::with_capacity(document.targets.len());
    for (index, target) in document.targets.iter().enumerate() {
        resolved.push(
            check_target_v2(catalog, &target.target_v2(), instance)
                .map_err(|(reason, path)| at(format!("/targets/{index}{path}"), reason))?,
        );
    }
    let projected =
        project_time_validity(catalog, facts, time).map_err(ResourceTargetsError::Evaluation)?;
    let conditions = document
        .targets
        .iter()
        .zip(&resolved)
        .map(|(target, (pool, fact_key))| {
            target_condition(&projected, instance, target, pool, fact_key, time)
        })
        .collect();
    let entries = document
        .entries
        .iter()
        .flatten()
        .map(|entry| entry_row(entry, None))
        .collect::<Result<Vec<_>, _>>()?;
    let policy_sha256 =
        composed_document(&document.instance, document.targets.clone(), &entries).identity()?;
    Ok(CheckedResourceTargets {
        policy_sha256,
        rows: Vec::new(),
        conditions,
        instance: document.instance.clone(),
        valid_until_unix_ms: None,
        document: CheckedDocument::V3(Box::new(document.clone())),
    })
}

/// What one target observes in the projection, as `ApplyResourceTargets` reports it.
fn target_condition(
    projected: &EvaluationFacts,
    instance: &InstanceSnapshot,
    target: &ResourceTargetSpecV3,
    pool: &PoolSpec,
    fact_key: &str,
    time: EvaluationTime,
) -> ResourceTargetCondition {
    let TargetCondition::AtLeast { amount } = target.condition;
    ResourceTargetCondition {
        target_id: target.id.clone(),
        resource: pool.id.clone(),
        fact_key: fact_key.to_owned(),
        state: match observe_target(projected, instance, pool, fact_key, time) {
            TargetObservation::Known {
                current,
                observed_at_unix_ms,
                ..
            } => ResourceTargetConditionState::Computed {
                current,
                observed_at_unix_ms,
                gap: amount.saturating_sub(current.unsigned_abs()),
            },
            TargetObservation::Pending { reason } => {
                ResourceTargetConditionState::AwaitingObservation { reason }
            }
        },
    }
}

/// An entry name: `[a-z0-9][a-z0-9_]*`, 1 to 64 bytes.
fn valid_entry_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_ENTRY_NAME_BYTES
        && name.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || (index > 0 && byte == b'_')
        })
}

/// The canonical text of an entry's value, written by the policy crate's canonical JSON writer
/// (`crate::canonical::canonical_serialized`: object keys sorted by UTF-16 code units, no
/// whitespace, ECMAScript-safe integers only), or why it is refused: `invalid_type` for a value
/// the writer refuses (a fraction, an integer beyond 2^53-1), `out_of_range` above 1024 bytes and
/// `invalid_value` for a text that holds a control character (the writer escapes U+0000 to
/// U+001F, not U+007F to U+009F, which no fact string may hold).
fn entry_value_text(value: &serde_json::Value) -> Result<String, ResourceTargetsRejectionReason> {
    use ResourceTargetsRejectionReason as Reason;
    let bytes = canonical_serialized(value).map_err(|_| Reason::InvalidType)?;
    let text = String::from_utf8(bytes).map_err(|_| Reason::InvalidType)?;
    if text.len() > MAX_RESOURCE_TARGET_ENTRY_VALUE_BYTES {
        return Err(Reason::OutOfRange);
    }
    if text.chars().any(char::is_control) {
        return Err(Reason::InvalidValue);
    }
    Ok(text)
}

/// One entry of a record being composed: its name and value, and its row (a stored one kept as
/// it is, or a new one).
#[derive(Debug, Clone, PartialEq, Eq)]
struct EntryRow {
    name: String,
    value: serde_json::Value,
    row: BTreeMap<String, ContractFactScalar>,
}

/// The `entry` row of a document's entry, set by `setter`; without a setter (an identity only)
/// the row holds the name and the value alone.
fn entry_row(
    entry: &ResourceTargetEntry,
    setter: Option<TargetSetter>,
) -> Result<EntryRow, ResourceTargetsError> {
    let text = entry_value_text(&entry.value).map_err(|_| {
        ResourceTargetsError::Internal(PolicyCode::ResourceTargetsCanonicalEncodeFailed.code())
    })?;
    let mut row = BTreeMap::from([
        (
            ROW_KIND.to_owned(),
            ContractFactScalar::String(ROW_ENTRY.to_owned()),
        ),
        (
            FIELD_NAME.to_owned(),
            ContractFactScalar::String(entry.name.clone()),
        ),
        (
            FIELD_VALUE_JSON.to_owned(),
            ContractFactScalar::String(text),
        ),
    ]);
    if let Some(setter) = setter {
        set_by(&mut row, setter);
    }
    Ok(EntryRow {
        name: entry.name.clone(),
        value: entry.value.clone(),
        row,
    })
}

/// Adds `set_unix_ms`, `channel` and, when the connection declared one, `initiator` to `row`.
fn set_by(row: &mut BTreeMap<String, ContractFactScalar>, setter: TargetSetter) {
    row.insert(
        FIELD_SET_AT.to_owned(),
        ContractFactScalar::Integer(i64::try_from(setter.set_unix_ms).unwrap_or(i64::MAX)),
    );
    row.insert(
        FIELD_CHANNEL.to_owned(),
        ContractFactScalar::String(setter.channel.as_str().to_owned()),
    );
    if let Some(initiator) = setter.initiator {
        row.insert(
            FIELD_INITIATOR.to_owned(),
            ContractFactScalar::String(initiator.as_str().to_owned()),
        );
    }
}

/// What a composed record holds: a v3 document, or a withdrawal, which is stored exactly as a v2
/// document with `targets: []` stores it.
enum ComposedDocument {
    Withdrawal(ResourceTargetsDocumentV2),
    V3(ResourceTargetsDocumentV3),
}

impl ComposedDocument {
    /// `sha256:<hex>` of the canonical serialization of the document.
    fn identity(&self) -> Result<String, ResourceTargetsError> {
        let canonical = match self {
            Self::Withdrawal(document) => canonical_serialized(document),
            Self::V3(document) => canonical_serialized(document),
        }
        .map_err(|_| {
            ResourceTargetsError::Internal(PolicyCode::ResourceTargetsCanonicalEncodeFailed.code())
        })?;
        Ok(format!("sha256:{:x}", Sha256::digest(canonical)))
    }
}

/// The document a record of `targets` and `entries` holds: with neither, the withdrawal.
fn composed_document(
    instance: &str,
    targets: Vec<ResourceTargetSpecV3>,
    entries: &[EntryRow],
) -> ComposedDocument {
    if targets.is_empty() && entries.is_empty() {
        return ComposedDocument::Withdrawal(ResourceTargetsDocumentV2 {
            schema_version: RESOURCE_TARGETS_SCHEMA_VERSION_V2.to_owned(),
            instance: instance.to_owned(),
            valid_until_unix_ms: None,
            targets: Vec::new(),
        });
    }
    ComposedDocument::V3(ResourceTargetsDocumentV3 {
        schema_version: RESOURCE_TARGETS_SCHEMA_VERSION_V3.to_owned(),
        instance: instance.to_owned(),
        targets,
        entries: (!entries.is_empty()).then(|| {
            entries
                .iter()
                .map(|entry| ResourceTargetEntry {
                    name: entry.name.clone(),
                    value: entry.value.clone(),
                })
                .collect()
        }),
        document_lifetime: None,
    })
}

impl CheckedResourceTargets {
    /// The record this checked document writes over the instance's active record `stored` (its
    /// rows; `None` without one), the rows a request writes set by `setter` (Workflow #455
    /// G-R2a, `contracts/resource-targets.md`, "Applying a document"):
    ///
    /// - a v1 or v2 document over no record, or over a record that is not a readable v3 record:
    ///   exactly the rows, identity and lifetime the check gave, as before G-R2a;
    /// - a v1 document over a v3 record: refused at `/schema_version` (`unsupported_schema_version`);
    /// - a v2 document over a v3 record: its targets, each with the document's lifetime as its
    ///   own and no setter, and the stored entries kept as they are; a v3 record;
    /// - a v3 document: its targets, set by `setter`; its `entries`, set by `setter`, when it
    ///   carries the member (an empty list included), else the stored entries of a v3 record kept
    ///   as they are;
    /// - with no target and no entry left, the withdrawal a v2 document with `targets: []` writes.
    pub fn compose(
        &self,
        stored: Option<&[BTreeMap<String, ContractFactScalar>]>,
        setter: TargetSetter,
    ) -> Result<ComposedResourceTargets, ResourceTargetsError> {
        let kept = stored.and_then(|rows| stored_entries(rows, &self.instance));
        let (targets, target_setter, entries) = match (&self.document, kept) {
            (CheckedDocument::V1(over_v3), Some(_)) => {
                return Err(ResourceTargetsError::Rejected(over_v3.clone()));
            }
            (CheckedDocument::V1(_) | CheckedDocument::V2(_), None) => {
                return Ok(ComposedResourceTargets {
                    policy_sha256: self.policy_sha256.clone(),
                    rows: self.rows.clone(),
                    expires_at_unix_ms: self.valid_until_unix_ms,
                    valid_until_unix_ms: self.valid_until_unix_ms,
                    conditions: self.conditions.clone(),
                });
            }
            (CheckedDocument::V2(document), Some(kept)) => (
                document
                    .targets
                    .iter()
                    .map(|target| ResourceTargetSpecV3::of_v2(target, document.valid_until_unix_ms))
                    .collect::<Vec<_>>(),
                None,
                kept,
            ),
            (CheckedDocument::V3(document), kept) => (
                document.targets.clone(),
                Some(setter),
                match &document.entries {
                    Some(entries) => entries
                        .iter()
                        .map(|entry| entry_row(entry, Some(setter)))
                        .collect::<Result<Vec<_>, _>>()?,
                    None => kept.unwrap_or_default(),
                },
            ),
        };
        let lifetimes = targets
            .iter()
            .map(|target| target.valid_until_unix_ms)
            .collect::<Option<Vec<_>>>();
        let valid_until_unix_ms = lifetimes.and_then(|lifetimes| lifetimes.into_iter().max());
        let document = composed_document(&self.instance, targets, &entries);
        let policy_sha256 = document.identity()?;
        let rows = match &document {
            ComposedDocument::Withdrawal(withdrawal) => encode_rows_v2(withdrawal, &policy_sha256),
            ComposedDocument::V3(document) => {
                encode_rows_v3(document, &policy_sha256, target_setter, entries)
            }
        };
        Ok(ComposedResourceTargets {
            policy_sha256,
            rows,
            expires_at_unix_ms: None,
            valid_until_unix_ms,
            conditions: if valid_until_unix_ms.is_some() {
                self.conditions.clone()
            } else {
                Vec::new()
            },
        })
    }
}

/// The rows of a v3 record: the `policy` header, one `target` row per target (a v2 target row,
/// its own `valid_until_unix_ms` when it has one, and `set_unix_ms`, `channel` and `initiator`
/// when `setter` set it), one `task` row per task reference, then the `entry` rows.
fn encode_rows_v3(
    document: &ResourceTargetsDocumentV3,
    policy_sha256: &str,
    setter: Option<TargetSetter>,
    entries: Vec<EntryRow>,
) -> Vec<BTreeMap<String, ContractFactScalar>> {
    let text = |value: &str| ContractFactScalar::String(value.to_owned());
    let integer =
        |value: u64| ContractFactScalar::Integer(i64::try_from(value).unwrap_or(i64::MAX));
    let mut rows = vec![BTreeMap::from([
        (ROW_KIND.to_owned(), text(ROW_POLICY)),
        ("schema_version".to_owned(), text(&document.schema_version)),
        ("instance".to_owned(), text(&document.instance)),
        ("policy_sha256".to_owned(), text(policy_sha256)),
    ])];
    for target in &document.targets {
        let TargetCondition::AtLeast { amount } = target.condition;
        let mut row = BTreeMap::from([
            (ROW_KIND.to_owned(), text(ROW_TARGET)),
            ("id".to_owned(), text(&target.id)),
            ("resource".to_owned(), text(&target.resource)),
            ("at_least".to_owned(), integer(amount)),
            ("mode".to_owned(), text(mode_name(target.apply.mode))),
            ("weight".to_owned(), text(weight_name(target.apply.weight))),
        ]);
        if let Some(scale) = target.scale {
            row.insert("scale".to_owned(), integer(scale));
        }
        if let Some(importance_milli) = target.importance_milli {
            row.insert("importance_milli".to_owned(), integer(importance_milli));
        }
        if let Some(rule) = target.rule {
            row.insert("rule".to_owned(), text(rule_name(rule)));
        }
        if let Some(manual_offset) = target.apply.manual_offset {
            row.insert(
                "manual_offset".to_owned(),
                text(manual_offset_name(manual_offset)),
            );
        }
        if let Some(valid_until) = target.valid_until_unix_ms {
            row.insert(FIELD_VALID_UNTIL.to_owned(), integer(valid_until));
        }
        if let Some(setter) = setter {
            set_by(&mut row, setter);
        }
        rows.push(row);
    }
    for target in &document.targets {
        for task in target.tasks.iter().flatten() {
            rows.push(BTreeMap::from([
                (ROW_KIND.to_owned(), text(ROW_TASK)),
                ("target".to_owned(), text(&target.id)),
                ("task".to_owned(), text(task)),
            ]));
        }
    }
    rows.extend(entries.into_iter().map(|entry| entry.row));
    rows
}

/// The entries of `stored` when it is a readable v3 record of `instance_id`, each with its row as
/// it is stored; `None` for any other record (a v1 or v2 record, a withdrawal, or one the v3
/// reader refuses, over which every document applies as over none).
fn stored_entries(
    stored: &[BTreeMap<String, ContractFactScalar>],
    instance_id: &str,
) -> Option<Vec<EntryRow>> {
    let rows = stored
        .iter()
        .map(|row| {
            row.iter()
                .map(|(field, value)| {
                    let value = match value {
                        ContractFactScalar::Boolean(value) => FactScalar::Boolean(*value),
                        ContractFactScalar::Integer(value) => FactScalar::Integer(*value),
                        ContractFactScalar::String(value) => FactScalar::String(value.clone()),
                        ContractFactScalar::TimestampMs(value) => FactScalar::TimestampMs(*value),
                        ContractFactScalar::DurationMs(value) => FactScalar::DurationMs(*value),
                    };
                    (field.clone(), value)
                })
                .collect::<BTreeMap<_, _>>()
        })
        .collect::<Vec<_>>();
    if !declares_v3_rows(&rows) {
        return None;
    }
    let decoded = decode_rows_v3(&rows, instance_id).ok()?;
    Some(
        decoded
            .entries
            .into_iter()
            .filter_map(|entry| {
                stored.get(entry.index).map(|row| EntryRow {
                    name: entry.name,
                    value: entry.value,
                    row: row.clone(),
                })
            })
            .collect(),
    )
}

/// Whether stored rows open with a `policy` header of the v3 version.
pub(super) fn declares_v3_rows(rows: &[BTreeMap<String, FactScalar>]) -> bool {
    rows.first().is_some_and(|header| {
        matches!(header.get(ROW_KIND), Some(FactScalar::String(kind)) if kind == ROW_POLICY)
            && matches!(
                header.get("schema_version"),
                Some(FactScalar::String(version)) if version == RESOURCE_TARGETS_SCHEMA_VERSION_V3
            )
    })
}

/// One target of a stored v3 record: a v2 target and its own lifetime.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DecodedTargetV3 {
    target: ResourceTargetSpecV2,
    valid_until_unix_ms: Option<u64>,
}

/// One entry of a stored v3 record and the position of its row.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DecodedEntryV3 {
    name: String,
    value: serde_json::Value,
    index: usize,
}

/// A stored v3 record read back from its rows.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DecodedResourceTargetsV3 {
    policy_sha256: String,
    targets: Vec<DecodedTargetV3>,
    entries: Vec<DecodedEntryV3>,
}

/// Strictly decodes the rows of `instance_id`'s stored v3 record. The header, the `target` rows
/// without their v3 fields and the `task` rows are read by the v2 reader, unchanged, so every v2
/// rule holds (the header's fields exactly, the identity's form, the instance, the targets' fields
/// and bounds, unique ids and resources, `manual_offset` only in override mode, the task rows);
/// a target's v3 fields are its own `valid_until_unix_ms` (a positive integer) and, together,
/// `set_unix_ms` (an integer, at least 0) and `channel` (a channel's word) with an optional
/// `initiator` (one of the six words); an `entry` row holds exactly `name` (of the entry form,
/// unique), `value_json` (a canonical text the entry value rule takes), `set_unix_ms`, `channel`
/// and an optional `initiator`, at most 96 of them. Anything else is `malformed_rows`, a header of
/// another version `unsupported_schema_version`, another instance `instance_mismatch`.
fn decode_rows_v3(
    rows: &[BTreeMap<String, FactScalar>],
    instance_id: &str,
) -> Result<DecodedResourceTargetsV3, RowsDecodeError> {
    use RowsDecodeError::{MalformedRows, UnsupportedSchemaVersion};
    let text = |row: &BTreeMap<String, FactScalar>, field: &str| match row.get(field) {
        Some(FactScalar::String(value)) => Ok(value.clone()),
        _ => Err(MalformedRows),
    };
    let (header, body) = rows.split_first().ok_or(MalformedRows)?;
    if text(header, ROW_KIND)? != ROW_POLICY {
        return Err(MalformedRows);
    }
    if text(header, "schema_version")? != RESOURCE_TARGETS_SCHEMA_VERSION_V3 {
        return Err(UnsupportedSchemaVersion);
    }
    let mut v2_header = header.clone();
    v2_header.insert(
        "schema_version".to_owned(),
        FactScalar::String(RESOURCE_TARGETS_SCHEMA_VERSION_V2.to_owned()),
    );
    let mut v2_rows = vec![v2_header];
    let mut lifetimes = Vec::new();
    let mut entries = Vec::new();
    let mut names = BTreeSet::new();
    for (offset, row) in body.iter().enumerate() {
        let kind = text(row, ROW_KIND)?;
        if kind == ROW_TARGET {
            let mut stripped = row.clone();
            let lifetime = match stripped.remove(FIELD_VALID_UNTIL) {
                None => None,
                Some(FactScalar::Integer(value)) if value >= 1 => {
                    Some(u64::try_from(value).map_err(|_| MalformedRows)?)
                }
                Some(_) => return Err(MalformedRows),
            };
            let set_at = stripped.remove(FIELD_SET_AT);
            let channel = stripped.remove(FIELD_CHANNEL);
            let initiator = stripped.remove(FIELD_INITIATOR);
            if (set_at.is_some() || channel.is_some() || initiator.is_some())
                && !set_fields(set_at.as_ref(), channel.as_ref(), initiator.as_ref())
            {
                return Err(MalformedRows);
            }
            lifetimes.push(lifetime);
            v2_rows.push(stripped);
        } else if kind == ROW_TASK {
            v2_rows.push(row.clone());
        } else if kind == ROW_ENTRY {
            let required = [
                ROW_KIND,
                FIELD_NAME,
                FIELD_VALUE_JSON,
                FIELD_SET_AT,
                FIELD_CHANNEL,
            ];
            if entries.len() >= MAX_RESOURCE_TARGET_ENTRIES
                || !required.iter().all(|field| row.contains_key(*field))
                || !row.keys().all(|field| {
                    required.contains(&field.as_str()) || field.as_str() == FIELD_INITIATOR
                })
                || !set_fields(
                    row.get(FIELD_SET_AT),
                    row.get(FIELD_CHANNEL),
                    row.get(FIELD_INITIATOR),
                )
            {
                return Err(MalformedRows);
            }
            let name = text(row, FIELD_NAME)?;
            let value_json = text(row, FIELD_VALUE_JSON)?;
            let value = serde_json::from_str::<serde_json::Value>(&value_json)
                .map_err(|_| MalformedRows)?;
            if !valid_entry_name(&name)
                || !names.insert(name.clone())
                || entry_value_text(&value).ok().as_deref() != Some(value_json.as_str())
            {
                return Err(MalformedRows);
            }
            entries.push(DecodedEntryV3 {
                name,
                value,
                index: offset + 1,
            });
        } else {
            return Err(MalformedRows);
        }
    }
    let DecodedResourceTargetsV2 {
        policy_sha256,
        targets,
    } = decode_rows_v2(&v2_rows, instance_id)?;
    if targets.len() != lifetimes.len() {
        return Err(MalformedRows);
    }
    Ok(DecodedResourceTargetsV3 {
        policy_sha256,
        targets: targets
            .into_iter()
            .zip(lifetimes)
            .map(|(target, valid_until_unix_ms)| DecodedTargetV3 {
                target,
                valid_until_unix_ms,
            })
            .collect(),
        entries,
    })
}

/// Whether `set_unix_ms`, `channel` and `initiator` form who set a row: an integer of at least 0,
/// one of the four channel words and, when present, one of the six initiator words.
fn set_fields(
    set_at: Option<&FactScalar>,
    channel: Option<&FactScalar>,
    initiator: Option<&FactScalar>,
) -> bool {
    let channels = [
        RuntimeChannel::Cli,
        RuntimeChannel::Mcp,
        RuntimeChannel::Ui,
        RuntimeChannel::Lab,
    ];
    matches!(set_at, Some(FactScalar::Integer(at)) if *at >= 0)
        && matches!(channel, Some(FactScalar::String(word))
            if channels.iter().any(|channel| channel.as_str() == word))
        && match initiator {
            None => true,
            Some(FactScalar::String(word)) => Initiator::from_word(word).is_some(),
            Some(_) => false,
        }
}

/// A stored v3 record as the evaluator reads it: unreadable when the v3 reader refuses its rows or
/// it carries an expiry (a v3 record is stored without one); no policy without a target; else the
/// targets whose own lifetime has not passed (`now <= valid_until_unix_ms`, or none) are an active
/// v2 policy's, whose lifetime is the earliest of theirs, and the others its lapsed targets; when
/// every target's lifetime has passed, an expired v2 policy whose lifetime is the latest of
/// theirs. Its version is the record's `policy_sha256` and `observed_at_unix_ms`, as a v2 record's.
pub(super) fn stored_policy_state_v3(
    fact: &ObservedFact,
    rows: &[BTreeMap<String, FactScalar>],
    instance_id: &str,
    time: EvaluationTime,
) -> Option<TargetPolicyState> {
    let unreadable = |code| Some(TargetPolicyState::Unreadable { code });
    let DecodedResourceTargetsV3 {
        policy_sha256,
        targets,
        ..
    } = match decode_rows_v3(rows, instance_id) {
        Ok(decoded) => decoded,
        Err(error) => return unreadable(error.code()),
    };
    if fact.expires_at_unix_ms.is_some() {
        return unreadable(RowsDecodeError::MalformedRows.code());
    }
    if targets.is_empty() {
        return None;
    }
    let applied_at_unix_ms = fact.observed_at_unix_ms;
    let (live, lapsed): (Vec<_>, Vec<_>) = targets.into_iter().partition(|target| {
        target
            .valid_until_unix_ms
            .is_none_or(|valid_until| time.unix_ms <= valid_until)
    });
    let lapsed = lapsed
        .into_iter()
        .map(|target| {
            target
                .valid_until_unix_ms
                .map(|valid_until_unix_ms| LapsedTarget {
                    target: target.target,
                    valid_until_unix_ms,
                })
        })
        .collect::<Option<Vec<_>>>();
    let Some(lapsed) = lapsed else {
        return unreadable(RowsDecodeError::MalformedRows.code());
    };
    if live.is_empty() {
        let Some(valid_until_unix_ms) =
            lapsed.iter().map(|target| target.valid_until_unix_ms).max()
        else {
            return unreadable(RowsDecodeError::MalformedRows.code());
        };
        return Some(TargetPolicyState::ExpiredV2 {
            policy_sha256,
            applied_at_unix_ms,
            valid_until_unix_ms,
            targets: lapsed.into_iter().map(|target| target.target).collect(),
        });
    }
    Some(TargetPolicyState::ActiveV2 {
        policy_sha256,
        applied_at_unix_ms,
        valid_until_unix_ms: live
            .iter()
            .filter_map(|target| target.valid_until_unix_ms)
            .min(),
        targets: live.into_iter().map(|target| target.target).collect(),
        lapsed,
    })
}

/// The targets of `instance_id`'s stored record in document form, each with its own lifetime,
/// when that record is a readable v3 record; `None` for any other record (`ResourceTargetView`).
pub(super) fn stored_v3_targets(
    facts: &EvaluationFacts,
    instance_id: &str,
) -> Option<Vec<ResourceTargetSpecV3>> {
    let record = facts.facts.iter().rfind(|fact| {
        fact.fact_key == RESOURCE_TARGETS_FACT_KEY
            && matches!(&fact.scope, ScopeSelector::Instance { instance_id: id } if id == instance_id)
    })?;
    let FactValue::RecordList(rows) = &record.value else {
        return None;
    };
    if !declares_v3_rows(rows) {
        return None;
    }
    let decoded = decode_rows_v3(rows, instance_id).ok()?;
    Some(
        decoded
            .targets
            .iter()
            .map(|target| ResourceTargetSpecV3::of_v2(&target.target, target.valid_until_unix_ms))
            .collect(),
    )
}

/// The tasks of `instance` that lapsed v3 targets covered, each with the lifetime of the first
/// such target in document order: a task whose scope covers the instance and one run of which
/// produces the target's pool, among the target's named tasks when it names some. An expired v2
/// policy names them on its candidates; under a v3 record whose other targets still weigh they
/// carry the same reason.
pub(super) fn lapsed_coverage(
    catalog: &CompiledCatalog,
    instance: &InstanceSnapshot,
    lapsed: &[LapsedTarget],
) -> BTreeMap<String, u64> {
    let bundle = catalog.catalog();
    let mut covered = BTreeMap::new();
    for lapsed in lapsed {
        let Some(pool) = bundle.pools.pools.iter().find(|pool| {
            pool.id == lapsed.target.resource
                && crate::evaluator::scope_matches_instance(&pool.scope, instance)
        }) else {
            continue;
        };
        for task in &bundle.tasks.tasks {
            if crate::evaluator::scope_matches_instance(&task.scope, instance)
                && super::per_run_production(task, &pool.id) >= 1
                && lapsed
                    .target
                    .tasks
                    .as_ref()
                    .is_none_or(|named| named.contains(&task.id))
            {
                covered
                    .entry(task.id.clone())
                    .or_insert(lapsed.valid_until_unix_ms);
            }
        }
    }
    covered
}
