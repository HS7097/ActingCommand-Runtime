// SPDX-License-Identifier: AGPL-3.0-only

//! Instance resource target policies (Workflow #308 RT-S1a/S1b, #335 S2b): the
//! `actingcommand.resource-targets.v1` and `.v2` documents, their formal parse and their check
//! against the active catalog and an authoritative projection, the record-list rows the Runtime
//! stores as the instance fact `session.resource_targets`, the pure target helpers the entry and
//! the evaluator share, and the evaluator's reading and scoring of stored policies: a v1 policy
//! scores its named tasks as before, a v2 policy weighs every produced resource of the instance
//! with its declared valuation plus its target's shortfall. Nothing here reads a clock, a file or
//! the network.

use std::collections::{BTreeMap, BTreeSet};

use actingcommand_contract::{
    FactScalar as ContractFactScalar, MAX_RESOURCE_TARGETS, MAX_RESOURCE_TARGETS_DOCUMENT_BYTES,
    RESOURCE_TARGETS_FACT_KEY, ResourceTargetCondition, ResourceTargetConditionState,
    ResourceTargetPendingReason, ResourceTargetsRejection, ResourceTargetsRejectionReason,
};
use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};

use crate::canonical::canonical_serialized;
use crate::evaluator::{activity_scope_specificity, project_time_validity, scope_matches_instance};
use crate::source::{CatalogDocumentSource, SourceMap, parse_document};
use crate::{
    CatalogDiagnostic, CatalogDiagnosticCode, CompiledCatalog, DecisionReason, EvaluationFacts,
    EvaluationTime, FactScalar, FactValue, InstanceSnapshot, MAX_ID_BYTES, ObservationRef,
    ObservedFact, PolicyEvaluationError, PolicyEvaluationResult, PoolSpec, PoolValueSource,
    SchedulingDocumentKind, ScopeSelector, TaskSpec,
};

pub const RESOURCE_TARGETS_SCHEMA_VERSION: &str = "actingcommand.resource-targets.v1";
/// The valuation-aware document (Workflow #335 S2b).
pub const RESOURCE_TARGETS_SCHEMA_VERSION_V2: &str = "actingcommand.resource-targets.v2";
/// Most tasks one target names.
pub const MAX_TASKS_PER_RESOURCE_TARGET: usize = 32;
/// Most task references one policy document holds.
pub const MAX_RESOURCE_TARGET_TASKS: usize = 128;
/// Upper bound of `importance_milli`, of a target weight and of a task target score.
pub const MAX_RESOURCE_TARGET_MILLI: u64 = 1_000_000;
/// Upper bound of `condition.amount` and `scale`: the canonical safe integer.
const MAX_RESOURCE_TARGET_AMOUNT: u64 = 9_007_199_254_740_991;
/// Longest policy lifetime: the fact TTL ceiling (one year).
const MAX_RESOURCE_TARGETS_VALIDITY_MS: u64 = actingcommand_contract::MAX_FACT_TTL_MS;
const MAX_REJECTION_FIELD_PATH_BYTES: usize = 4096;
const DOCUMENT_SOURCE_URI: &str = "memory://resource-targets.json";

/// The `actingcommand.resource-targets.v1` document of one instance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceTargetsDocument {
    pub schema_version: String,
    pub instance: String,
    /// Required when `targets` is non-empty; must be absent when it is empty (a withdrawal).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_u64"
    )]
    pub valid_until_unix_ms: Option<u64>,
    pub targets: Vec<ResourceTargetSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceTargetSpec {
    pub id: String,
    /// A pool id of the active catalog.
    pub resource: String,
    pub condition: TargetCondition,
    /// The gap at which the target weight equals `importance_milli`.
    pub scale: u64,
    pub importance_milli: u64,
    pub rule: TargetRule,
    pub apply: TargetApplication,
    pub tasks: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TargetCondition {
    AtLeast { amount: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetRule {
    ShortfallLinear,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetApplication {
    pub mode: TargetMode,
    pub weight: TargetWeight,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetMode {
    Adjust,
    Override,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetWeight {
    ScoreStage,
}

fn present_u64<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<u64>, D::Error> {
    u64::deserialize(deserializer).map(Some)
}

/// The `actingcommand.resource-targets.v2` document of one instance (Workflow #335 S2b). It
/// keeps the v1 fields; a target's `scale`, `importance_milli`, `rule` and `tasks` may be left
/// out and are then resolved from the pool's `valuation` in every evaluation, and an override
/// may state whether it keeps the manual offset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceTargetsDocumentV2 {
    pub schema_version: String,
    pub instance: String,
    /// Required when `targets` is non-empty; must be absent when it is empty (a withdrawal).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub valid_until_unix_ms: Option<u64>,
    pub targets: Vec<ResourceTargetSpecV2>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceTargetSpecV2 {
    pub id: String,
    /// A pool id of the active catalog, named by at most one target of the document.
    pub resource: String,
    pub condition: TargetCondition,
    /// The gap step `S`; absent takes the pool's `valuation.scale`.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub scale: Option<u64>,
    /// `I`, extra milli per produced step for each missing step `S`; absent takes the pool's
    /// `valuation.gap.weight_milli`.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub importance_milli: Option<u64>,
    /// Absent takes the pool's `valuation.gap.rule`, or `shortfall_linear` without a gap block.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub rule: Option<TargetRule>,
    pub apply: TargetApplicationV2,
    /// Required in `override` mode; absent covers every candidate of the instance that
    /// produces the resource.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub tasks: Option<Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetApplicationV2 {
    pub mode: TargetMode,
    pub weight: TargetWeight,
    /// Only in `override` mode; absent is `keep`.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub manual_offset: Option<ManualOffset>,
}

/// Whether an effective v2 override keeps the candidate's manual offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManualOffset {
    Keep,
    Supersede,
}

/// An optional field that, when present, holds a value: an explicit `null` is a type mismatch.
fn present<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(deserializer).map(Some)
}

/// A structurally valid document with its source positions.
#[derive(Debug)]
pub struct ParsedResourceTargets {
    /// The v1 document; for a v2 document only its header (`schema_version`, `instance`,
    /// `valid_until_unix_ms`) without targets.
    document: ResourceTargetsDocument,
    /// The v2 document; `None` for a v1 one.
    document_v2: Option<ResourceTargetsDocumentV2>,
    source_map: SourceMap,
}

impl ParsedResourceTargets {
    /// The v1 document as parsed. A v2 document shows here only its header, without targets:
    /// read either version through [`Self::instance`] and [`Self::valid_until_unix_ms`].
    pub fn document(&self) -> &ResourceTargetsDocument {
        &self.document
    }

    /// The instance the document names, for either version.
    pub fn instance(&self) -> &str {
        &self.document.instance
    }

    /// The policy lifetime, for either version; `None` exactly for a withdrawal.
    pub fn valid_until_unix_ms(&self) -> Option<u64> {
        self.document.valid_until_unix_ms
    }
}

/// A document checked against the active catalog and an authoritative projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedResourceTargets {
    /// `sha256:<hex>` of the canonical serialization of the document.
    pub policy_sha256: String,
    /// The stored record-list rows: one `policy` header, one `target` row per target and one
    /// `task` row per task reference, in document order.
    pub rows: Vec<BTreeMap<String, ContractFactScalar>>,
    /// What each target observes in the projection, in document order.
    pub conditions: Vec<ResourceTargetCondition>,
}

/// Why a document cannot be applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceTargetsError {
    /// The document is refused at one field; nothing is recorded.
    Rejected(Box<ResourceTargetsRejection>),
    /// The time-validity projection shared with the evaluator failed.
    Evaluation(PolicyEvaluationError),
    /// A result the parser or the canonical encoder cannot produce for a valid input.
    Internal(&'static str),
}

/// Parses the raw document with the scheduling declaration parser and checks everything that
/// needs neither the catalog nor the projection: size, JSON and field types, schema version,
/// bounds and identifier charset, uniqueness, and whether `valid_until_unix_ms` belongs.
/// A document whose top-level `schema_version` is exactly the v2 string takes the v2 checks
/// (Workflow #335 S2b); every other document takes the v1 path unchanged. Takes no lock and
/// reads nothing but `bytes`.
pub fn parse_resource_targets(bytes: &[u8]) -> Result<ParsedResourceTargets, ResourceTargetsError> {
    use ResourceTargetsRejectionReason as Reason;
    if bytes.len() > MAX_RESOURCE_TARGETS_DOCUMENT_BYTES {
        return Err(rejected(String::new(), 1, 1, Reason::OutOfRange));
    }
    let source = CatalogDocumentSource::new(DOCUMENT_SOURCE_URI, bytes.to_vec());
    if declares_v2(bytes) {
        return parse_resource_targets_v2(&source);
    }
    let parsed =
        parse_document::<ResourceTargetsDocument>(&source, SchedulingDocumentKind::ResourceTargets)
            .map_err(|diagnostic| declaration_rejection(*diagnostic))?;
    let parsed = ParsedResourceTargets {
        document: parsed.value,
        document_v2: None,
        source_map: parsed.source_map,
    };
    let document = &parsed.document;
    let at = |path: String, reason| rejected_at(&parsed.source_map, path, reason);
    if document.schema_version != RESOURCE_TARGETS_SCHEMA_VERSION {
        return Err(at(
            "/schema_version".to_owned(),
            Reason::UnsupportedSchemaVersion,
        ));
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
        if !(1..=MAX_RESOURCE_TARGET_AMOUNT).contains(&target.scale) {
            return Err(at(format!("{path}/scale"), Reason::OutOfRange));
        }
        if !(1..=MAX_RESOURCE_TARGET_MILLI).contains(&target.importance_milli) {
            return Err(at(format!("{path}/importance_milli"), Reason::OutOfRange));
        }
        if !(1..=MAX_TASKS_PER_RESOURCE_TARGET).contains(&target.tasks.len()) {
            return Err(at(format!("{path}/tasks"), Reason::OutOfRange));
        }
        for task_index in 0..target.tasks.len() {
            task_references += 1;
            if task_references > MAX_RESOURCE_TARGET_TASKS {
                return Err(at(format!("{path}/tasks/{task_index}"), Reason::OutOfRange));
            }
        }
    }
    let mut target_ids = BTreeSet::new();
    let mut task_ids = BTreeSet::new();
    for (index, target) in document.targets.iter().enumerate() {
        if !target_ids.insert(target.id.as_str()) {
            return Err(at(format!("/targets/{index}/id"), Reason::DuplicateId));
        }
        for (task_index, task) in target.tasks.iter().enumerate() {
            if !task_ids.insert(task.as_str()) {
                return Err(at(
                    format!("/targets/{index}/tasks/{task_index}"),
                    Reason::DuplicateTask,
                ));
            }
        }
    }
    match (document.targets.is_empty(), document.valid_until_unix_ms) {
        (false, None) => {
            return Err(at("/valid_until_unix_ms".to_owned(), Reason::MissingField));
        }
        (true, Some(_)) => {
            return Err(at("/valid_until_unix_ms".to_owned(), Reason::InvalidValue));
        }
        _ => {}
    }
    Ok(parsed)
}

/// The version probe (Workflow #335 S2b): whether the document's top-level `schema_version` is
/// exactly the v2 string. It refuses nothing; a document it cannot read takes the v1 path,
/// whose parse reports it, so every non-v2 document meets the v1 rejections unchanged.
pub(crate) fn declares_v2(bytes: &[u8]) -> bool {
    serde_json::from_slice::<serde_json::Value>(bytes).is_ok_and(|document| {
        document
            .get("schema_version")
            .and_then(serde_json::Value::as_str)
            == Some(RESOURCE_TARGETS_SCHEMA_VERSION_V2)
    })
}

/// A declaration parser refusal as a document rejection.
fn declaration_rejection(diagnostic: CatalogDiagnostic) -> ResourceTargetsError {
    use ResourceTargetsRejectionReason as Reason;
    let reason = match diagnostic.code {
        CatalogDiagnosticCode::InvalidJson => Reason::InvalidJson,
        CatalogDiagnosticCode::DuplicateKey => Reason::DuplicateKey,
        CatalogDiagnosticCode::UnknownField => Reason::UnknownField,
        CatalogDiagnosticCode::MissingRequiredField => Reason::MissingField,
        CatalogDiagnosticCode::TypeMismatch => Reason::InvalidType,
        _ => {
            return ResourceTargetsError::Internal("resource_targets_parser_code_unexpected");
        }
    };
    rejected(
        diagnostic.json_path,
        diagnostic.source.line,
        diagnostic.source.column,
        reason,
    )
}

/// The v2 half of [`parse_resource_targets`], on the source it built: the declaration parse,
/// then bounds and identifier charset, uniqueness (target ids, tasks, resources), the apply
/// rules and whether `valid_until_unix_ms` belongs.
fn parse_resource_targets_v2(
    source: &CatalogDocumentSource,
) -> Result<ParsedResourceTargets, ResourceTargetsError> {
    use ResourceTargetsRejectionReason as Reason;
    let parsed = parse_document::<ResourceTargetsDocumentV2>(
        source,
        SchedulingDocumentKind::ResourceTargets,
    )
    .map_err(|diagnostic| declaration_rejection(*diagnostic))?;
    let (document, source_map) = (parsed.value, parsed.source_map);
    let at = |path: String, reason| rejected_at(&source_map, path, reason);
    if document.schema_version != RESOURCE_TARGETS_SCHEMA_VERSION_V2 {
        return Err(at(
            "/schema_version".to_owned(),
            Reason::UnsupportedSchemaVersion,
        ));
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
    match (document.targets.is_empty(), document.valid_until_unix_ms) {
        (false, None) => {
            return Err(at("/valid_until_unix_ms".to_owned(), Reason::MissingField));
        }
        (true, Some(_)) => {
            return Err(at("/valid_until_unix_ms".to_owned(), Reason::InvalidValue));
        }
        _ => {}
    }
    Ok(ParsedResourceTargets {
        document: ResourceTargetsDocument {
            schema_version: document.schema_version.clone(),
            instance: document.instance.clone(),
            valid_until_unix_ms: document.valid_until_unix_ms,
            targets: Vec::new(),
        },
        document_v2: Some(document),
        source_map,
    })
}

/// Checks a parsed document against the active catalog and an authoritative projection
/// (`facts`) at `time`: the instance, the policy lifetime, then per target its resource and
/// its tasks. On success returns the policy identity, the rows to store and the current
/// condition of every target. Pure: reads nothing but its arguments.
pub fn check_resource_targets(
    parsed: &ParsedResourceTargets,
    catalog: &CompiledCatalog,
    facts: &EvaluationFacts,
    time: EvaluationTime,
) -> Result<CheckedResourceTargets, ResourceTargetsError> {
    use ResourceTargetsRejectionReason as Reason;
    if let Some(document) = &parsed.document_v2 {
        return check_resource_targets_v2(parsed, document, catalog, facts, time);
    }
    let document = &parsed.document;
    let at = |path: String, reason| rejected_at(&parsed.source_map, path, reason);
    let instance = facts
        .instances
        .iter()
        .find(|instance| instance.instance_id == document.instance)
        .ok_or_else(|| at("/instance".to_owned(), Reason::UnknownInstance))?;
    if let Some(valid_until) = document.valid_until_unix_ms
        && (valid_until <= time.unix_ms
            || valid_until
                > time
                    .unix_ms
                    .saturating_add(MAX_RESOURCE_TARGETS_VALIDITY_MS))
    {
        return Err(at(
            "/valid_until_unix_ms".to_owned(),
            Reason::ValidityOutOfRange,
        ));
    }
    let mut resolved = Vec::with_capacity(document.targets.len());
    for (index, target) in document.targets.iter().enumerate() {
        resolved.push(
            resolve_target(catalog, target, instance)
                .map_err(|(reason, path)| at(format!("/targets/{index}{path}"), reason))?,
        );
    }
    let canonical = canonical_serialized(document)
        .map_err(|_| ResourceTargetsError::Internal("resource_targets_canonical_encode_failed"))?;
    let policy_sha256 = format!("sha256:{:x}", Sha256::digest(canonical));
    let projected =
        project_time_validity(catalog, facts, time).map_err(ResourceTargetsError::Evaluation)?;
    let conditions = document
        .targets
        .iter()
        .zip(&resolved)
        .map(|(target, resolved)| {
            let TargetCondition::AtLeast { amount } = target.condition;
            ResourceTargetCondition {
                target_id: target.id.clone(),
                resource: resolved.pool.id.clone(),
                fact_key: resolved.fact_key.clone(),
                state: match observe_target(
                    &projected,
                    instance,
                    resolved.pool,
                    &resolved.fact_key,
                    time,
                ) {
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
        })
        .collect();
    Ok(CheckedResourceTargets {
        rows: encode_rows(document, &policy_sha256),
        policy_sha256,
        conditions,
    })
}

/// The v2 half of [`check_resource_targets`]: the instance, the policy lifetime, then per
/// target its resource, whether its step and importance resolve, and its tasks.
fn check_resource_targets_v2(
    parsed: &ParsedResourceTargets,
    document: &ResourceTargetsDocumentV2,
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
    if let Some(valid_until) = document.valid_until_unix_ms
        && (valid_until <= time.unix_ms
            || valid_until
                > time
                    .unix_ms
                    .saturating_add(MAX_RESOURCE_TARGETS_VALIDITY_MS))
    {
        return Err(at(
            "/valid_until_unix_ms".to_owned(),
            Reason::ValidityOutOfRange,
        ));
    }
    let mut resolved = Vec::with_capacity(document.targets.len());
    for (index, target) in document.targets.iter().enumerate() {
        resolved.push(
            check_target_v2(catalog, target, instance)
                .map_err(|(reason, path)| at(format!("/targets/{index}{path}"), reason))?,
        );
    }
    let canonical = canonical_serialized(document)
        .map_err(|_| ResourceTargetsError::Internal("resource_targets_canonical_encode_failed"))?;
    let policy_sha256 = format!("sha256:{:x}", Sha256::digest(canonical));
    let projected =
        project_time_validity(catalog, facts, time).map_err(ResourceTargetsError::Evaluation)?;
    let conditions = document
        .targets
        .iter()
        .zip(&resolved)
        .map(|(target, (pool, fact_key))| {
            let TargetCondition::AtLeast { amount } = target.condition;
            ResourceTargetCondition {
                target_id: target.id.clone(),
                resource: pool.id.clone(),
                fact_key: fact_key.clone(),
                state: match observe_target(&projected, instance, pool, fact_key, time) {
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
        })
        .collect();
    Ok(CheckedResourceTargets {
        rows: encode_rows_v2(document, &policy_sha256),
        policy_sha256,
        conditions,
    })
}

/// The entry's check of one v2 target: its pool, then its step and importance (the target's
/// own or the pool valuation's), then its named tasks, or, without `tasks`, whether any task of
/// the instance produces the pool. A refusal names its reason and its path below the target.
fn check_target_v2<'a>(
    catalog: &'a CompiledCatalog,
    target: &ResourceTargetSpecV2,
    instance: &InstanceSnapshot,
) -> Result<(&'a PoolSpec, String), (ResourceTargetsRejectionReason, String)> {
    use ResourceTargetsRejectionReason as Reason;
    let (pool, fact_key) = resolve_target_pool(catalog, &target.resource, instance)?;
    match target_terms(target, pool) {
        Err(MissingTerm::Scale) => return Err((Reason::MissingField, "/scale".to_owned())),
        Err(MissingTerm::Importance) => {
            return Err((Reason::MissingField, "/importance_milli".to_owned()));
        }
        Ok(_) => {}
    }
    match &target.tasks {
        Some(tasks) => {
            for (index, task_id) in tasks.iter().enumerate() {
                resolve_target_task(catalog, pool, task_id, instance)
                    .map_err(|reason| (reason, format!("/tasks/{index}")))?;
            }
        }
        None => {
            let produced = catalog.catalog().tasks.tasks.iter().any(|task| {
                task_on_instance(task, instance).is_ok() && per_run_production(task, &pool.id) >= 1
            });
            if !produced {
                return Err((Reason::UnmappedTask, "/resource".to_owned()));
            }
        }
    }
    Ok((pool, fact_key))
}

/// A v2 target term that resolves to nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MissingTerm {
    /// No `scale` on the target and no `valuation` on the pool.
    Scale,
    /// No `importance_milli` on the target and no `valuation.gap` on the pool.
    Importance,
}

/// A v2 target's gap step `S` and importance `I`: the target's own, else the pool valuation's
/// `scale` and `gap.weight_milli`. The rule needs no resolution: `shortfall_linear`, the only
/// one, is also what an absent rule and an absent gap block mean.
fn target_terms(target: &ResourceTargetSpecV2, pool: &PoolSpec) -> Result<(u64, u64), MissingTerm> {
    let valuation = pool.valuation.as_ref();
    let scale = target
        .scale
        .or_else(|| valuation.map(|valuation| valuation.scale))
        .ok_or(MissingTerm::Scale)?;
    let importance = target
        .importance_milli
        .or_else(|| {
            valuation
                .and_then(|valuation| valuation.gap.as_ref())
                .map(|gap| u64::from(gap.weight_milli))
        })
        .ok_or(MissingTerm::Importance)?;
    Ok((scale, importance))
}

/// One target resolved against a catalog for one instance: its pool, the pool's inventory
/// fact key and, per named task, the expected effective production of one run (`r_k`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedTarget<'a> {
    pub(crate) pool: &'a PoolSpec,
    pub(crate) fact_key: String,
    pub(crate) per_task: Vec<(String, u128)>,
}

/// Resolves `target` for `instance`: its pool, then each task, the first refusal winning.
/// The entry check and the evaluator share both halves, so both judge a target alike. A
/// refusal names its reason and its path below the target.
pub(crate) fn resolve_target<'a>(
    catalog: &'a CompiledCatalog,
    target: &ResourceTargetSpec,
    instance: &InstanceSnapshot,
) -> Result<ResolvedTarget<'a>, (ResourceTargetsRejectionReason, String)> {
    let (pool, fact_key) = resolve_target_pool(catalog, &target.resource, instance)?;
    let mut per_task = Vec::with_capacity(target.tasks.len());
    for (index, task_id) in target.tasks.iter().enumerate() {
        let per_run = resolve_target_task(catalog, pool, task_id, instance)
            .map_err(|reason| (reason, format!("/tasks/{index}")))?;
        per_task.push((task_id.clone(), per_run));
    }
    Ok(ResolvedTarget {
        pool,
        fact_key,
        per_task,
    })
}

/// The pool half of [`resolve_target`]: the target's pool `resource` and its inventory fact
/// key, or the refusal at `/resource`.
pub(crate) fn resolve_target_pool<'a>(
    catalog: &'a CompiledCatalog,
    resource: &str,
    instance: &InstanceSnapshot,
) -> Result<(&'a PoolSpec, String), (ResourceTargetsRejectionReason, String)> {
    use ResourceTargetsRejectionReason as Reason;
    let pool = catalog
        .catalog()
        .pools
        .pools
        .iter()
        .find(|pool| pool.id == resource)
        .ok_or_else(|| (Reason::UnknownResource, "/resource".to_owned()))?;
    let ObservationRef::Fact { fact_key } = &pool.observation else {
        return Err((Reason::ResourceNotObservable, "/resource".to_owned()));
    };
    if !(fact_key.starts_with("resource.") || fact_key.starts_with("inventory.")) {
        return Err((Reason::ResourceNotObservable, "/resource".to_owned()));
    }
    if !scope_matches_instance(&pool.scope, instance) {
        return Err((Reason::ResourceOutOfScope, "/resource".to_owned()));
    }
    Ok((pool, fact_key.clone()))
}

/// The task half of [`resolve_target`]: one run's expected effective production of `pool`
/// in milli-units (`r_k > 0`) by `task_id` on `instance`, or why it cannot serve the target.
pub(crate) fn resolve_target_task(
    catalog: &CompiledCatalog,
    pool: &PoolSpec,
    task_id: &str,
    instance: &InstanceSnapshot,
) -> Result<u128, ResourceTargetsRejectionReason> {
    let task = target_task(catalog, task_id, instance)?;
    let per_run = per_run_production(task, &pool.id);
    if per_run == 0 {
        return Err(ResourceTargetsRejectionReason::UnmappedTask);
    }
    Ok(per_run)
}

/// One run's production in milli-units. Integer declarations retain their established
/// `floor(amount * confidence / 1000)` contribution; explicit expectations are already
/// probability/batch adjusted and are not multiplied by evidence confidence.
fn per_run_production(task: &TaskSpec, pool_id: &str) -> u128 {
    task.produces
        .iter()
        .filter(|effect| effect.pool_id == pool_id)
        .map(|effect| match effect.expected_amount_milli {
            Some(amount) => u128::from(amount),
            None => {
                u128::from(effect.amount.expect("compiled integer effect"))
                    * u128::from(effect.confidence_milli)
                    / 1_000
                    * 1_000
            }
        })
        // At most 128 effects, each a canonical integer times 1000: this fits u128.
        .sum()
}

/// The catalog task `task_id` when it exists, its scope covers `instance` and no instance
/// override of `instance` disables it.
fn target_task<'a>(
    catalog: &'a CompiledCatalog,
    task_id: &str,
    instance: &InstanceSnapshot,
) -> Result<&'a TaskSpec, ResourceTargetsRejectionReason> {
    use ResourceTargetsRejectionReason as Reason;
    let task = catalog
        .catalog()
        .tasks
        .tasks
        .iter()
        .find(|task| task.id == task_id)
        .ok_or(Reason::UnknownTask)?;
    task_on_instance(task, instance)?;
    Ok(task)
}

/// Whether `task` can run on `instance`: its scope covers it and no instance override of
/// `instance` disables it.
fn task_on_instance(
    task: &TaskSpec,
    instance: &InstanceSnapshot,
) -> Result<(), ResourceTargetsRejectionReason> {
    use ResourceTargetsRejectionReason as Reason;
    if !scope_matches_instance(&task.scope, instance) {
        return Err(Reason::TaskOutOfScope);
    }
    if task
        .instance_overrides
        .iter()
        .any(|entry| entry.instance_id == instance.instance_id && entry.enabled.0 == Some(false))
    {
        return Err(Reason::TaskDisabled);
    }
    Ok(())
}

/// A target's inventory as the time-validity projection shows it to one instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TargetObservation {
    Known {
        current: i64,
        observed_at_unix_ms: u64,
        fresh_until_unix_ms: Option<u64>,
    },
    Pending {
        reason: ResourceTargetPendingReason,
    },
}

/// Reads the most specific inline record of the target's inventory fact that `instance` can
/// see in `projected_facts` (already time-validity projected). Known needs a non-negative
/// integer, a positive confidence (at least the pool's minimum for a ledger-fact pool) and an
/// unexpired record; otherwise the first failing reason in the order missing,
/// low_confidence, expired, invalid_value. The entry check and the evaluator share it.
pub(crate) fn observe_target(
    projected_facts: &EvaluationFacts,
    instance: &InstanceSnapshot,
    pool: &PoolSpec,
    fact_key: &str,
    time: EvaluationTime,
) -> TargetObservation {
    use ResourceTargetPendingReason as Reason;
    let fact = projected_facts
        .facts
        .iter()
        .filter(|fact| fact.fact_key == fact_key && scope_matches_instance(&fact.scope, instance))
        .max_by_key(|fact| activity_scope_specificity(&fact.scope));
    let Some(fact) = fact else {
        return TargetObservation::Pending {
            reason: Reason::Missing,
        };
    };
    let minimum_confidence = match pool.value_source {
        PoolValueSource::LedgerFact {
            minimum_confidence_milli,
        } => minimum_confidence_milli.max(1),
        PoolValueSource::StaticSnapshot => 1,
    };
    if fact.confidence_milli < minimum_confidence {
        return TargetObservation::Pending {
            reason: Reason::LowConfidence,
        };
    }
    if fact
        .expires_at_unix_ms
        .is_some_and(|expires| time.unix_ms > expires)
    {
        return TargetObservation::Pending {
            reason: Reason::Expired,
        };
    }
    match fact.value {
        FactValue::Integer(current) if current >= 0 => TargetObservation::Known {
            current,
            observed_at_unix_ms: fact.observed_at_unix_ms,
            fresh_until_unix_ms: fact.expires_at_unix_ms,
        },
        _ => TargetObservation::Pending {
            reason: Reason::InvalidValue,
        },
    }
}

const ROW_KIND: &str = "row";
const ROW_POLICY: &str = "policy";
const ROW_TARGET: &str = "target";
const ROW_TASK: &str = "task";

/// Encodes a checked document as the stored rows: the `policy` header, then one `target` row
/// per target and one `task` row per task reference, both in document order.
pub(crate) fn encode_rows(
    document: &ResourceTargetsDocument,
    policy_sha256: &str,
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
        rows.push(BTreeMap::from([
            (ROW_KIND.to_owned(), text(ROW_TARGET)),
            ("id".to_owned(), text(&target.id)),
            ("resource".to_owned(), text(&target.resource)),
            ("at_least".to_owned(), integer(amount)),
            ("scale".to_owned(), integer(target.scale)),
            (
                "importance_milli".to_owned(),
                integer(target.importance_milli),
            ),
            ("rule".to_owned(), text(rule_name(target.rule))),
            ("mode".to_owned(), text(mode_name(target.apply.mode))),
            ("weight".to_owned(), text(weight_name(target.apply.weight))),
        ]));
    }
    for target in &document.targets {
        for task in &target.tasks {
            rows.push(BTreeMap::from([
                (ROW_KIND.to_owned(), text(ROW_TASK)),
                ("target".to_owned(), text(&target.id)),
                ("task".to_owned(), text(task)),
            ]));
        }
    }
    rows
}

/// The optional fields of a v2 `target` row, stored exactly when the document states them.
const ROW_TARGET_V2_OPTIONAL: [&str; 4] = ["scale", "importance_milli", "rule", "manual_offset"];

/// Encodes a checked v2 document as the stored rows: the `policy` header, one `target` row per
/// target with the optional fields it states, and one `task` row per task reference; a target
/// without `tasks` has no task row.
pub(crate) fn encode_rows_v2(
    document: &ResourceTargetsDocumentV2,
    policy_sha256: &str,
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
    rows
}

/// A stored policy read back from its rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DecodedResourceTargets {
    pub(crate) policy_sha256: String,
    pub(crate) targets: Vec<ResourceTargetSpec>,
}

/// Why stored rows cannot be read; the evaluator degrades only that instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RowsDecodeError {
    UnsupportedSchemaVersion,
    MalformedRows,
    InstanceMismatch,
}

impl RowsDecodeError {
    pub(crate) const fn code(self) -> &'static str {
        match self {
            Self::UnsupportedSchemaVersion => "unsupported_schema_version",
            Self::MalformedRows => "malformed_rows",
            Self::InstanceMismatch => "instance_mismatch",
        }
    }
}

/// Strictly decodes the rows of `instance_id`'s stored policy: exactly one leading header of
/// a known version naming that instance, known row kinds with exactly their fields and
/// types, bounded values, scheduling identifiers for target, resource and task ids, unique
/// targets, and task rows that name an earlier target and reference each task once.
pub(crate) fn decode_rows(
    rows: &[BTreeMap<String, FactScalar>],
    instance_id: &str,
) -> Result<DecodedResourceTargets, RowsDecodeError> {
    use RowsDecodeError::{InstanceMismatch, MalformedRows, UnsupportedSchemaVersion};
    let text = |row: &BTreeMap<String, FactScalar>, field: &str| match row.get(field) {
        Some(FactScalar::String(value)) => Ok(value.clone()),
        _ => Err(MalformedRows),
    };
    let amount =
        |row: &BTreeMap<String, FactScalar>, field: &str, maximum: u64| match row.get(field) {
            Some(FactScalar::Integer(value)) => u64::try_from(*value)
                .ok()
                .filter(|value| (1..=maximum).contains(value))
                .ok_or(MalformedRows),
            _ => Err(MalformedRows),
        };
    let fields = |row: &BTreeMap<String, FactScalar>, expected: &[&str]| {
        row.len() == expected.len() && expected.iter().all(|field| row.contains_key(*field))
    };
    let (header, body) = rows.split_first().ok_or(MalformedRows)?;
    if text(header, ROW_KIND)? != ROW_POLICY {
        return Err(MalformedRows);
    }
    if text(header, "schema_version")? != RESOURCE_TARGETS_SCHEMA_VERSION {
        return Err(UnsupportedSchemaVersion);
    }
    if !fields(
        header,
        &[ROW_KIND, "schema_version", "instance", "policy_sha256"],
    ) {
        return Err(MalformedRows);
    }
    let policy_sha256 = text(header, "policy_sha256")?;
    if policy_sha256.strip_prefix("sha256:").is_none_or(|digest| {
        digest.len() != 64
            || !digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    }) {
        return Err(MalformedRows);
    }
    if text(header, "instance")? != instance_id {
        return Err(InstanceMismatch);
    }
    let mut targets: Vec<ResourceTargetSpec> = Vec::new();
    let mut tasks = BTreeSet::new();
    for row in body {
        match text(row, ROW_KIND)?.as_str() {
            ROW_TARGET => {
                if !fields(
                    row,
                    &[
                        ROW_KIND,
                        "id",
                        "resource",
                        "at_least",
                        "scale",
                        "importance_milli",
                        "rule",
                        "mode",
                        "weight",
                    ],
                ) || targets.len() >= MAX_RESOURCE_TARGETS
                {
                    return Err(MalformedRows);
                }
                let id = text(row, "id")?;
                let resource = text(row, "resource")?;
                if !valid_identifier(&id)
                    || !valid_identifier(&resource)
                    || targets.iter().any(|target| target.id == id)
                {
                    return Err(MalformedRows);
                }
                targets.push(ResourceTargetSpec {
                    id,
                    resource,
                    condition: TargetCondition::AtLeast {
                        amount: amount(row, "at_least", MAX_RESOURCE_TARGET_AMOUNT)?,
                    },
                    scale: amount(row, "scale", MAX_RESOURCE_TARGET_AMOUNT)?,
                    importance_milli: amount(row, "importance_milli", MAX_RESOURCE_TARGET_MILLI)?,
                    rule: match text(row, "rule")?.as_str() {
                        "shortfall_linear" => TargetRule::ShortfallLinear,
                        _ => return Err(MalformedRows),
                    },
                    apply: TargetApplication {
                        mode: match text(row, "mode")?.as_str() {
                            "adjust" => TargetMode::Adjust,
                            "override" => TargetMode::Override,
                            _ => return Err(MalformedRows),
                        },
                        weight: match text(row, "weight")?.as_str() {
                            "score_stage" => TargetWeight::ScoreStage,
                            _ => return Err(MalformedRows),
                        },
                    },
                    tasks: Vec::new(),
                });
            }
            ROW_TASK => {
                if !fields(row, &[ROW_KIND, "target", "task"]) {
                    return Err(MalformedRows);
                }
                let target_id = text(row, "target")?;
                let task = text(row, "task")?;
                let target = targets
                    .iter_mut()
                    .find(|target| target.id == target_id)
                    .ok_or(MalformedRows)?;
                if !valid_identifier(&task)
                    || target.tasks.len() >= MAX_TASKS_PER_RESOURCE_TARGET
                    || tasks.len() >= MAX_RESOURCE_TARGET_TASKS
                    || !tasks.insert(task.clone())
                {
                    return Err(MalformedRows);
                }
                target.tasks.push(task);
            }
            _ => return Err(MalformedRows),
        }
    }
    if targets.iter().any(|target| target.tasks.is_empty()) {
        return Err(MalformedRows);
    }
    Ok(DecodedResourceTargets {
        policy_sha256,
        targets,
    })
}

/// Whether stored rows open with a `policy` header of the v2 version; every other record list
/// takes the v1 reader unchanged.
fn declares_v2_rows(rows: &[BTreeMap<String, FactScalar>]) -> bool {
    rows.first().is_some_and(|header| {
        matches!(header.get(ROW_KIND), Some(FactScalar::String(kind)) if kind == ROW_POLICY)
            && matches!(
                header.get("schema_version"),
                Some(FactScalar::String(version)) if version == RESOURCE_TARGETS_SCHEMA_VERSION_V2
            )
    })
}

/// A stored v2 policy read back from its rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DecodedResourceTargetsV2 {
    pub(crate) policy_sha256: String,
    pub(crate) targets: Vec<ResourceTargetSpecV2>,
}

/// Strictly decodes the rows of `instance_id`'s stored v2 policy: exactly one leading header of
/// the v2 version naming that instance, known row kinds with exactly their required fields and
/// types plus only the listed optional ones, bounded values, scheduling identifiers for target,
/// resource and task ids, unique targets and resources, `manual_offset` only in override mode,
/// task rows that name an earlier target and reference each task once, and at least one task
/// row for every override target.
pub(crate) fn decode_rows_v2(
    rows: &[BTreeMap<String, FactScalar>],
    instance_id: &str,
) -> Result<DecodedResourceTargetsV2, RowsDecodeError> {
    use RowsDecodeError::{InstanceMismatch, MalformedRows, UnsupportedSchemaVersion};
    let text = |row: &BTreeMap<String, FactScalar>, field: &str| match row.get(field) {
        Some(FactScalar::String(value)) => Ok(value.clone()),
        _ => Err(MalformedRows),
    };
    let amount =
        |row: &BTreeMap<String, FactScalar>, field: &str, maximum: u64| match row.get(field) {
            Some(FactScalar::Integer(value)) => u64::try_from(*value)
                .ok()
                .filter(|value| (1..=maximum).contains(value))
                .ok_or(MalformedRows),
            _ => Err(MalformedRows),
        };
    let optional_amount = |row: &BTreeMap<String, FactScalar>, field: &str, maximum: u64| {
        row.contains_key(field)
            .then(|| amount(row, field, maximum))
            .transpose()
    };
    let (header, body) = rows.split_first().ok_or(MalformedRows)?;
    if text(header, ROW_KIND)? != ROW_POLICY {
        return Err(MalformedRows);
    }
    if text(header, "schema_version")? != RESOURCE_TARGETS_SCHEMA_VERSION_V2 {
        return Err(UnsupportedSchemaVersion);
    }
    let header_fields = [ROW_KIND, "schema_version", "instance", "policy_sha256"];
    if header.len() != header_fields.len()
        || !header_fields
            .iter()
            .all(|field| header.contains_key(*field))
    {
        return Err(MalformedRows);
    }
    let policy_sha256 = text(header, "policy_sha256")?;
    if policy_sha256.strip_prefix("sha256:").is_none_or(|digest| {
        digest.len() != 64
            || !digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    }) {
        return Err(MalformedRows);
    }
    if text(header, "instance")? != instance_id {
        return Err(InstanceMismatch);
    }
    let required = [ROW_KIND, "id", "resource", "at_least", "mode", "weight"];
    let mut targets: Vec<ResourceTargetSpecV2> = Vec::new();
    let mut tasks = BTreeSet::new();
    for row in body {
        match text(row, ROW_KIND)?.as_str() {
            ROW_TARGET => {
                if !required.iter().all(|field| row.contains_key(*field))
                    || !row.keys().all(|field| {
                        required.contains(&field.as_str())
                            || ROW_TARGET_V2_OPTIONAL.contains(&field.as_str())
                    })
                    || targets.len() >= MAX_RESOURCE_TARGETS
                {
                    return Err(MalformedRows);
                }
                let id = text(row, "id")?;
                let resource = text(row, "resource")?;
                if !valid_identifier(&id)
                    || !valid_identifier(&resource)
                    || targets
                        .iter()
                        .any(|target| target.id == id || target.resource == resource)
                {
                    return Err(MalformedRows);
                }
                let mode = match text(row, "mode")?.as_str() {
                    "adjust" => TargetMode::Adjust,
                    "override" => TargetMode::Override,
                    _ => return Err(MalformedRows),
                };
                let manual_offset = match row.get("manual_offset") {
                    None => None,
                    Some(_) if mode == TargetMode::Adjust => return Err(MalformedRows),
                    Some(_) => Some(match text(row, "manual_offset")?.as_str() {
                        "keep" => ManualOffset::Keep,
                        "supersede" => ManualOffset::Supersede,
                        _ => return Err(MalformedRows),
                    }),
                };
                targets.push(ResourceTargetSpecV2 {
                    id,
                    resource,
                    condition: TargetCondition::AtLeast {
                        amount: amount(row, "at_least", MAX_RESOURCE_TARGET_AMOUNT)?,
                    },
                    scale: optional_amount(row, "scale", MAX_RESOURCE_TARGET_AMOUNT)?,
                    importance_milli: optional_amount(
                        row,
                        "importance_milli",
                        MAX_RESOURCE_TARGET_MILLI,
                    )?,
                    rule: match row.get("rule") {
                        None => None,
                        Some(_) => match text(row, "rule")?.as_str() {
                            "shortfall_linear" => Some(TargetRule::ShortfallLinear),
                            _ => return Err(MalformedRows),
                        },
                    },
                    apply: TargetApplicationV2 {
                        mode,
                        weight: match text(row, "weight")?.as_str() {
                            "score_stage" => TargetWeight::ScoreStage,
                            _ => return Err(MalformedRows),
                        },
                        manual_offset,
                    },
                    tasks: None,
                });
            }
            ROW_TASK => {
                if row.len() != 3 || !row.contains_key("target") || !row.contains_key("task") {
                    return Err(MalformedRows);
                }
                let target_id = text(row, "target")?;
                let task = text(row, "task")?;
                let target = targets
                    .iter_mut()
                    .find(|target| target.id == target_id)
                    .ok_or(MalformedRows)?;
                let named = target.tasks.get_or_insert_with(Vec::new);
                if !valid_identifier(&task)
                    || named.len() >= MAX_TASKS_PER_RESOURCE_TARGET
                    || tasks.len() >= MAX_RESOURCE_TARGET_TASKS
                    || !tasks.insert(task.clone())
                {
                    return Err(MalformedRows);
                }
                named.push(task);
            }
            _ => return Err(MalformedRows),
        }
    }
    if targets
        .iter()
        .any(|target| target.apply.mode == TargetMode::Override && target.tasks.is_none())
    {
        return Err(MalformedRows);
    }
    Ok(DecodedResourceTargetsV2 {
        policy_sha256,
        targets,
    })
}

/// T1 task target score `min(⌊g·I·u / (S·U)⌋, 1_000_000)` in exact integer arithmetic, and
/// whether the cap applied; `None` for zero denominators or arithmetic overflow.
pub(crate) fn task_target_milli(
    gap: u64,
    importance_milli: u64,
    scale: u64,
    useful: u128,
    best_useful: u128,
) -> Option<(u64, bool)> {
    if scale == 0 || best_useful == 0 {
        return None;
    }
    let common = greatest_common_divisor(useful, best_useful);
    let numerator =
        (u128::from(gap) * u128::from(importance_milli)).checked_mul(useful / common)?;
    let quotient = numerator.checked_div(u128::from(scale).checked_mul(best_useful / common)?)?;
    let cap = u128::from(MAX_RESOURCE_TARGET_MILLI);
    Some(if quotient > cap {
        (MAX_RESOURCE_TARGET_MILLI, true)
    } else {
        (u64::try_from(quotient).ok()?, false)
    })
}

fn greatest_common_divisor(mut left: u128, mut right: u128) -> u128 {
    while right != 0 {
        (left, right) = (right, left % right);
    }
    left
}

/// Add an exact score fraction. Reduction keeps ordinary catalog scales small; a
/// fraction outside u128 is an explicit evaluation failure, never a rounded zero.
fn add_score_fraction(
    sum: &mut (u128, u128),
    numerator: u128,
    denominator: u128,
) -> Result<(), &'static str> {
    const OVERFLOW: &str = "expected production score fraction overflow";
    if denominator == 0 {
        return Err("resource valuation step is zero");
    }
    let common = greatest_common_divisor(numerator, denominator);
    let (numerator, denominator) = (numerator / common, denominator / common);
    let common = greatest_common_divisor(sum.1, denominator);
    let left = sum.0.checked_mul(denominator / common).ok_or(OVERFLOW)?;
    let right = numerator.checked_mul(sum.1 / common).ok_or(OVERFLOW)?;
    let numerator = left.checked_add(right).ok_or(OVERFLOW)?;
    let denominator = sum.1.checked_mul(denominator / common).ok_or(OVERFLOW)?;
    let common = greatest_common_divisor(numerator, denominator);
    *sum = (numerator / common, denominator / common);
    Ok(())
}

fn quantity_units(milli: u128) -> String {
    if milli.is_multiple_of(1_000) {
        (milli / 1_000).to_string()
    } else {
        format!("{}.{:03}", milli / 1_000, milli % 1_000)
    }
}

/// One instance's own stored policy as the evaluator reads it (Workflow #308 RT-S1b).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TargetPolicyState {
    Active {
        policy_sha256: String,
        applied_at_unix_ms: u64,
        valid_until_unix_ms: u64,
        targets: Vec<ResourceTargetSpec>,
    },
    /// Past `valid_until_unix_ms`; the targets only name the candidates it no longer scores.
    Expired {
        policy_sha256: String,
        applied_at_unix_ms: u64,
        valid_until_unix_ms: u64,
        targets: Vec<ResourceTargetSpec>,
    },
    Unreadable {
        code: &'static str,
    },
    /// An active v2 policy (Workflow #335 S2b): the only state that enables resource weights.
    ActiveV2 {
        policy_sha256: String,
        applied_at_unix_ms: u64,
        valid_until_unix_ms: u64,
        targets: Vec<ResourceTargetSpecV2>,
    },
    /// A v2 policy past `valid_until_unix_ms`: it weighs nothing and only names the candidates
    /// it would have weighed.
    ExpiredV2 {
        policy_sha256: String,
        applied_at_unix_ms: u64,
        valid_until_unix_ms: u64,
        targets: Vec<ResourceTargetSpecV2>,
    },
}

/// What the evaluator knows about one instance's resource target policy.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct InstanceTargetPolicy {
    /// The instance-scoped record; `None` without one or for a withdrawal (`targets: []`).
    pub(crate) state: Option<TargetPolicyState>,
    /// A server- or game-scoped record of the key covering the instance, which only an
    /// ordinary publication before RT-S1a could have written: `(scope kind, observed_at)`.
    pub(crate) ignored: Option<(&'static str, u64)>,
}

/// Reads every stored `session.resource_targets` record of `facts` as given, before the
/// time-validity projection, so a timeline reset never silently drops a policy. An
/// instance-scoped record that cannot be decoded, or that holds targets without an expiry, is
/// `Unreadable`; a withdrawal is no policy; `applied_at` is the record's `observed_at` and its
/// confidence is not read. A server- or game-scoped record is never a policy and never an
/// error: every instance it covers notes it as ignored (a server record over a game one). The
/// map holds only the instances with something to report.
pub(crate) fn instance_target_policies(
    facts: &EvaluationFacts,
    time: EvaluationTime,
) -> PolicyEvaluationResult<BTreeMap<String, InstanceTargetPolicy>> {
    let mut policies = BTreeMap::<String, InstanceTargetPolicy>::new();
    for fact in facts
        .facts
        .iter()
        .filter(|fact| fact.fact_key == RESOURCE_TARGETS_FACT_KEY)
    {
        let scope_kind = match &fact.scope {
            ScopeSelector::Instance { instance_id } => {
                if let Some(state) = stored_policy_state(fact, instance_id, time) {
                    policies.entry(instance_id.clone()).or_default().state = Some(state);
                }
                continue;
            }
            ScopeSelector::Server { .. } => "server",
            ScopeSelector::Game { .. } => "game",
        };
        for instance in facts
            .instances
            .iter()
            .filter(|instance| scope_matches_instance(&fact.scope, instance))
        {
            let entry = policies.entry(instance.instance_id.clone()).or_default();
            if entry.ignored.is_none() || scope_kind == "server" {
                entry.ignored = Some((scope_kind, fact.observed_at_unix_ms));
            }
        }
    }
    Ok(policies)
}

fn stored_policy_state(
    fact: &ObservedFact,
    instance_id: &str,
    time: EvaluationTime,
) -> Option<TargetPolicyState> {
    let unreadable = |code| Some(TargetPolicyState::Unreadable { code });
    let FactValue::RecordList(rows) = &fact.value else {
        return unreadable(RowsDecodeError::MalformedRows.code());
    };
    if declares_v2_rows(rows) {
        return stored_policy_state_v2(fact, rows, instance_id, time);
    }
    let decoded = match decode_rows(rows, instance_id) {
        Ok(decoded) => decoded,
        Err(error) => return unreadable(error.code()),
    };
    if decoded.targets.is_empty() {
        return None;
    }
    let Some(valid_until_unix_ms) = fact.expires_at_unix_ms else {
        return unreadable(RowsDecodeError::MalformedRows.code());
    };
    let DecodedResourceTargets {
        policy_sha256,
        targets,
    } = decoded;
    let applied_at_unix_ms = fact.observed_at_unix_ms;
    Some(if time.unix_ms > valid_until_unix_ms {
        TargetPolicyState::Expired {
            policy_sha256,
            applied_at_unix_ms,
            valid_until_unix_ms,
            targets,
        }
    } else {
        TargetPolicyState::Active {
            policy_sha256,
            applied_at_unix_ms,
            valid_until_unix_ms,
            targets,
        }
    })
}

/// [`stored_policy_state`] for a v2 record: unreadable when its rows are refused or it holds
/// targets without an expiry, no policy for a withdrawal, else active or expired.
fn stored_policy_state_v2(
    fact: &ObservedFact,
    rows: &[BTreeMap<String, FactScalar>],
    instance_id: &str,
    time: EvaluationTime,
) -> Option<TargetPolicyState> {
    let unreadable = |code| Some(TargetPolicyState::Unreadable { code });
    let DecodedResourceTargetsV2 {
        policy_sha256,
        targets,
    } = match decode_rows_v2(rows, instance_id) {
        Ok(decoded) => decoded,
        Err(error) => return unreadable(error.code()),
    };
    if targets.is_empty() {
        return None;
    }
    let Some(valid_until_unix_ms) = fact.expires_at_unix_ms else {
        return unreadable(RowsDecodeError::MalformedRows.code());
    };
    let applied_at_unix_ms = fact.observed_at_unix_ms;
    Some(if time.unix_ms > valid_until_unix_ms {
        TargetPolicyState::ExpiredV2 {
            policy_sha256,
            applied_at_unix_ms,
            valid_until_unix_ms,
            targets,
        }
    } else {
        TargetPolicyState::ActiveV2 {
            policy_sha256,
            applied_at_unix_ms,
            valid_until_unix_ms,
            targets,
        }
    })
}

/// What a stored policy does to one score-stage candidate (Workflow #308 RT-S1b).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TargetEffect {
    /// The instance has no policy, or no target of it names the task.
    None,
    /// The instance's stored policy cannot be read; every candidate of the instance.
    PolicyUnreadable {
        code: &'static str,
    },
    /// The instance's policy has expired; only the candidates it names.
    PolicyExpired {
        policy_sha256: String,
        applied_at_unix_ms: u64,
        valid_until_unix_ms: u64,
    },
    /// The target's pool, or this task, no longer maps in the active catalog.
    Unmapped {
        id: String,
        resource: String,
        task: String,
        why: &'static str,
    },
    /// The inventory has no usable observation (time-validity projected facts).
    Pending {
        id: String,
        fact_key: String,
        reason: ResourceTargetPendingReason,
    },
    /// The gap is zero: the target has no effect and an override is released.
    Satisfied {
        id: String,
        policy_sha256: String,
        applied_at_unix_ms: u64,
        current: i64,
        at_least: u64,
        mode: TargetMode,
    },
    Applied(Box<AppliedTarget>),
    /// What an active v2 policy adds to the candidate (Workflow #335 S2b); every candidate of
    /// the instance meets one, with a zero term when nothing of it is weighed.
    Resources(Box<ResourceTerm>),
}

/// The resource term `R` of one candidate under an active v2 policy, with everything its
/// reasons show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResourceTerm {
    /// `R = min(Σ T, 1_000_000)`, added to the effective score.
    pub(crate) term_milli: u64,
    capped: bool,
    /// The effective override covering the candidate: its target id and offset switch.
    pub(crate) override_target: Option<(String, ManualOffset)>,
    /// The policy lifetime and the expiry of every inventory observation behind an applied
    /// target that weighs the candidate: they bound its freshness when it takes part.
    pub(crate) valid_until_unix_ms: u64,
    pub(crate) inventory_expiries: Vec<u64>,
    policy_sha256: String,
    applied_at_unix_ms: u64,
    /// One `resource_targets` item per target covering the candidate, in document order.
    targets: Vec<String>,
    /// `(target, resource, task, why)` when a target names the task but the task, or the
    /// target's resource, no longer maps.
    unmapped: Option<(String, String, String, &'static str)>,
    /// One item per weighed resource, by term descending, then pool id.
    items: Vec<WeightItem>,
}

/// One weighed resource of a candidate: `T = ⌊r · W / Q⌋` with `W = B + Γ`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct WeightItem {
    pool: String,
    per_run: u128,
    step: u64,
    base_milli: u64,
    gap_weight_milli: u64,
    effective_milli: u64,
    term_milli: u128,
}

impl ResourceTerm {
    /// Whether the policy moves the candidate: a positive term or an effective override.
    pub(crate) fn takes_part(&self) -> bool {
        self.term_milli > 0 || self.override_target.is_some()
    }

    /// The candidate's v2 reasons after the policy-level one, in this order and each at most
    /// once: `resource_targets`, `resource_target_unmapped:<id>`,
    /// `resource_target_override:<id>`, `resource_weights`.
    fn reasons(&self, score_milli: Option<i64>, offset_milli: i64) -> Vec<DecisionReason> {
        let mut reasons = Vec::with_capacity(4);
        if !self.targets.is_empty() {
            reasons.push(DecisionReason {
                code: "resource_targets".to_owned(),
                detail: bounded_list(String::new(), &self.targets, "; "),
            });
        }
        if let Some((id, resource, task, why)) = &self.unmapped {
            reasons.push(DecisionReason {
                code: format!("resource_target_unmapped:{id}"),
                detail: format!("resource={resource} task={task} why={why}"),
            });
        }
        if let Some((id, manual_offset)) = &self.override_target {
            let score = score_milli.map_or_else(|| "none".to_owned(), |score| score.to_string());
            reasons.push(DecisionReason {
                code: format!("resource_target_override:{id}"),
                detail: match manual_offset {
                    ManualOffset::Keep => format!(
                        "superseded score={score}; offset={offset_milli} kept (manual_offset=keep); utility kept"
                    ),
                    ManualOffset::Supersede => format!(
                        "superseded score={score} offset={offset_milli} (manual_offset=supersede); utility kept"
                    ),
                },
            });
        }
        if !self.items.is_empty() {
            let items = self
                .items
                .iter()
                .map(|item| {
                    format!(
                        "{}:r={},per={},base={},gap={},effective={},term={}",
                        item.pool,
                        quantity_units(item.per_run),
                        item.step,
                        item.base_milli,
                        item.gap_weight_milli,
                        item.effective_milli,
                        item.term_milli
                    )
                })
                .collect::<Vec<_>>();
            reasons.push(DecisionReason {
                code: "resource_weights".to_owned(),
                detail: bounded_list(
                    format!(
                        "policy={}@{} term={}{} items=",
                        self.policy_sha256,
                        self.applied_at_unix_ms,
                        self.term_milli,
                        if self.capped { " capped" } else { "" }
                    ),
                    &items,
                    ";",
                ),
            });
        }
        reasons
    }
}

/// `prefix` followed by `items` joined with `separator`, counted while joining so the detail
/// takes at most 1010 bytes; the items that do not fit fold into `+<n>more`.
fn bounded_list(prefix: String, items: &[String], separator: &str) -> String {
    const DETAIL_BUDGET_BYTES: usize = 1_010;
    let mut detail = prefix;
    for (index, item) in items.iter().enumerate() {
        let joint = if index == 0 { "" } else { separator };
        let rest = items.len() - index - 1;
        // Room for the fold marker should a later item not fit.
        let reserve = if rest == 0 {
            0
        } else {
            separator.len() + format!("+{rest}more").len()
        };
        if detail.len() + joint.len() + item.len() + reserve > DETAIL_BUDGET_BYTES {
            detail.push_str(joint);
            detail.push_str(&format!("+{}more", items.len() - index));
            break;
        }
        detail.push_str(joint);
        detail.push_str(item);
    }
    detail
}

/// An applied target with every T1 intermediate it recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AppliedTarget {
    pub(crate) id: String,
    pub(crate) mode: TargetMode,
    /// `s_k`, added to (adjust) or replacing score and offset in (override) the effective score.
    pub(crate) task_target_milli: u64,
    pub(crate) valid_until_unix_ms: u64,
    pub(crate) inventory_expires_at_unix_ms: Option<u64>,
    capped: bool,
    policy_sha256: String,
    applied_at_unix_ms: u64,
    resource: String,
    fact_key: String,
    current: i64,
    at_least: u64,
    gap: u64,
    scale: u64,
    importance_milli: u64,
    weight_milli: u64,
    per_run: u128,
    useful: u128,
    best_useful: u128,
}

impl TargetEffect {
    pub(crate) fn applied(&self) -> Option<&AppliedTarget> {
        match self {
            Self::Applied(applied) => Some(applied),
            _ => None,
        }
    }

    /// The candidate's resource term under an active v2 policy.
    pub(crate) fn resources(&self) -> Option<&ResourceTerm> {
        match self {
            Self::Resources(term) => Some(term),
            _ => None,
        }
    }

    /// The target-level reason: applied, satisfied, pending or unmapped.
    fn target_reason(&self) -> Option<DecisionReason> {
        let (code, detail) = match self {
            Self::None
            | Self::PolicyUnreadable { .. }
            | Self::PolicyExpired { .. }
            | Self::Resources(_) => return None,
            Self::Unmapped {
                id,
                resource,
                task,
                why,
            } => (
                format!("resource_target_unmapped:{id}"),
                format!("resource={resource} task={task} why={why}"),
            ),
            Self::Pending {
                id,
                fact_key,
                reason,
            } => (
                format!("resource_target_pending:{id}"),
                format!(
                    "fact_key={fact_key} reason={}; configuration applied, waiting for a valid observation",
                    pending_name(*reason)
                ),
            ),
            Self::Satisfied {
                id,
                policy_sha256,
                applied_at_unix_ms,
                current,
                at_least,
                mode,
            } => (
                format!("resource_target_satisfied:{id}"),
                format!(
                    "policy={policy_sha256} applied_at={applied_at_unix_ms} current={current} at_least={at_least} gap=0 mode={}{}",
                    mode_name(*mode),
                    if *mode == TargetMode::Override {
                        "; override released"
                    } else {
                        ""
                    }
                ),
            ),
            Self::Applied(applied) => (
                format!("resource_target_applied:{}", applied.id),
                format!(
                    "policy={} applied_at={} resource={} fact_key={} mode={} current={} at_least={} gap={} scale={} importance={} weight={} per_run={} useful={} best_useful={} task_target={}{}",
                    applied.policy_sha256,
                    applied.applied_at_unix_ms,
                    applied.resource,
                    applied.fact_key,
                    mode_name(applied.mode),
                    applied.current,
                    applied.at_least,
                    applied.gap,
                    applied.scale,
                    applied.importance_milli,
                    applied.weight_milli,
                    quantity_units(applied.per_run),
                    quantity_units(applied.useful),
                    quantity_units(applied.best_useful),
                    applied.task_target_milli,
                    if applied.capped { " capped" } else { "" }
                ),
            ),
        };
        Some(DecisionReason { code, detail })
    }
}

/// One instance's stored policy resolved once per evaluation against the active catalog and
/// the time-validity projected facts.
#[derive(Debug)]
pub(crate) struct InstanceTargets {
    ignored: Option<(&'static str, u64)>,
    policy: ResolvedPolicy,
}

#[derive(Debug)]
enum ResolvedPolicy {
    None,
    Unreadable(&'static str),
    Expired {
        policy_sha256: String,
        applied_at_unix_ms: u64,
        valid_until_unix_ms: u64,
        named: BTreeSet<String>,
    },
    Active {
        /// The effect of every named task that can have a candidate on the instance.
        effects: BTreeMap<String, TargetEffect>,
        /// `(target, task, why)` of named tasks that can have no candidate on the instance
        /// (unknown, out of scope or disabled), in target and task order.
        unevaluable: Vec<(String, String, &'static str)>,
    },
    ActiveV2(Box<ResolvedV2>),
}

/// An active v2 policy resolved for one instance (Workflow #335 S2b).
#[derive(Debug)]
struct ResolvedV2 {
    /// The resource term of every catalog task whose scope covers the instance.
    terms: BTreeMap<String, ResourceTerm>,
    /// The `resource_target_tasks_unevaluable` items, in target order: as for v1 each named
    /// task that can have no candidate on the instance (`<target>/<task>:<why>`), and each
    /// target without `tasks` that lapsed at catalog level (`<target>@<pool>:<why>`): its
    /// pool no longer resolves, or no task of the instance produces it.
    unevaluable: Vec<String>,
    /// The term of a candidate without an entry above: nothing weighed.
    empty: ResourceTerm,
}

/// One v2 target resolved for one instance, once per evaluation.
struct ResolvedTargetV2<'a> {
    target: &'a ResourceTargetSpecV2,
    pool: &'a PoolSpec,
    /// The named tasks that map (`r >= 1`); `None` covers every producing candidate.
    named: Option<BTreeSet<&'a str>>,
    status: TargetStatus,
}

/// What a resolved v2 target contributes this evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TargetStatus {
    /// Step and importance resolved and the gap is open: `Γ = min(⌊I·g/S⌋, 1e6)`.
    Applied {
        current: i64,
        gap: u64,
        step: u64,
        importance_milli: u64,
        gap_weight_milli: u64,
        capped: bool,
        inventory_expires_at_unix_ms: Option<u64>,
    },
    Satisfied {
        current: i64,
    },
    Pending {
        reason: ResourceTargetPendingReason,
    },
    /// A left-out step or importance whose pool no longer declares it.
    Unresolved {
        why: &'static str,
    },
}

impl ResolvedTargetV2<'_> {
    /// Whether the target covers `task`, which produces `per_run` of its pool per run.
    fn covers(&self, task: &str, per_run: u128) -> bool {
        match &self.named {
            Some(named) => named.contains(task),
            None => per_run >= 1,
        }
    }

    /// `Γ` of a covered candidate.
    fn gap_weight_milli(&self) -> u64 {
        match self.status {
            TargetStatus::Applied {
                gap_weight_milli, ..
            } => gap_weight_milli,
            _ => 0,
        }
    }

    /// The target's `resource_targets` item.
    fn item(&self) -> String {
        let (id, pool) = (&self.target.id, &self.pool.id);
        let TargetCondition::AtLeast { amount: at_least } = self.target.condition;
        match self.status {
            TargetStatus::Applied {
                current,
                gap,
                step,
                importance_milli,
                gap_weight_milli,
                capped,
                ..
            } => format!(
                "{id}@{pool}:applied current={current} at_least={at_least} gap={gap} step={step} importance={importance_milli} gap_weight={gap_weight_milli}{}",
                if capped { " capped" } else { "" }
            ),
            TargetStatus::Satisfied { current } => format!(
                "{id}@{pool}:satisfied current={current} at_least={at_least} gap=0 gap_weight=0"
            ),
            TargetStatus::Pending { reason } => format!(
                "{id}@{pool}:pending reason={} gap_weight=pending",
                pending_name(reason)
            ),
            TargetStatus::Unresolved { why } => {
                format!("{id}@{pool}:unresolved why={why} gap_weight=0")
            }
        }
    }
}

/// What resolving one target of an active policy reads.
struct TargetContext<'a> {
    catalog: &'a CompiledCatalog,
    projected_facts: &'a EvaluationFacts,
    instance: &'a InstanceSnapshot,
    time: EvaluationTime,
    policy_sha256: &'a str,
    applied_at_unix_ms: u64,
    valid_until_unix_ms: u64,
}

/// Resolves `policy` for `instance`: each target's pool once, each named task once, and the
/// inventory once per target in `projected_facts`. The error names a state only a bug reaches.
pub(crate) fn resolve_instance_targets(
    catalog: &CompiledCatalog,
    projected_facts: &EvaluationFacts,
    instance: &InstanceSnapshot,
    policy: &InstanceTargetPolicy,
    time: EvaluationTime,
) -> Result<InstanceTargets, &'static str> {
    let resolved = match &policy.state {
        None => ResolvedPolicy::None,
        Some(TargetPolicyState::Unreadable { code }) => ResolvedPolicy::Unreadable(code),
        Some(TargetPolicyState::Expired {
            policy_sha256,
            applied_at_unix_ms,
            valid_until_unix_ms,
            targets,
        }) => ResolvedPolicy::Expired {
            policy_sha256: policy_sha256.clone(),
            applied_at_unix_ms: *applied_at_unix_ms,
            valid_until_unix_ms: *valid_until_unix_ms,
            named: targets
                .iter()
                .flat_map(|target| target.tasks.iter().cloned())
                .collect(),
        },
        Some(TargetPolicyState::Active {
            policy_sha256,
            applied_at_unix_ms,
            valid_until_unix_ms,
            targets,
        }) => {
            let context = TargetContext {
                catalog,
                projected_facts,
                instance,
                time,
                policy_sha256,
                applied_at_unix_ms: *applied_at_unix_ms,
                valid_until_unix_ms: *valid_until_unix_ms,
            };
            let mut effects = BTreeMap::new();
            let mut unevaluable = Vec::new();
            for target in targets {
                target_effects(&context, target, &mut effects, &mut unevaluable)?;
            }
            ResolvedPolicy::Active {
                effects,
                unevaluable,
            }
        }
        Some(TargetPolicyState::ExpiredV2 {
            policy_sha256,
            applied_at_unix_ms,
            valid_until_unix_ms,
            targets,
        }) => ResolvedPolicy::Expired {
            policy_sha256: policy_sha256.clone(),
            applied_at_unix_ms: *applied_at_unix_ms,
            valid_until_unix_ms: *valid_until_unix_ms,
            named: weighed_tasks(catalog, instance, targets),
        },
        Some(TargetPolicyState::ActiveV2 {
            policy_sha256,
            applied_at_unix_ms,
            valid_until_unix_ms,
            targets,
        }) => ResolvedPolicy::ActiveV2(Box::new(resolve_v2(
            &TargetContext {
                catalog,
                projected_facts,
                instance,
                time,
                policy_sha256,
                applied_at_unix_ms: *applied_at_unix_ms,
                valid_until_unix_ms: *valid_until_unix_ms,
            },
            targets,
        )?)),
    };
    Ok(InstanceTargets {
        ignored: policy.ignored,
        policy: resolved,
    })
}

/// The tasks of `instance` that an active v2 policy with `targets` would weigh: those producing
/// a pool that covers the instance and either declares a valuation or is the resource of a
/// target covering the task. An expired policy names exactly them.
fn weighed_tasks(
    catalog: &CompiledCatalog,
    instance: &InstanceSnapshot,
    targets: &[ResourceTargetSpecV2],
) -> BTreeSet<String> {
    let bundle = catalog.catalog();
    bundle
        .tasks
        .tasks
        .iter()
        .filter(|task| scope_matches_instance(&task.scope, instance))
        .filter(|task| {
            bundle.pools.pools.iter().any(|pool| {
                scope_matches_instance(&pool.scope, instance)
                    && per_run_production(task, &pool.id) >= 1
                    && (pool.valuation.is_some()
                        || targets.iter().any(|target| {
                            target.resource == pool.id
                                && target
                                    .tasks
                                    .as_ref()
                                    .is_none_or(|named| named.contains(&task.id))
                        }))
            })
        })
        .map(|task| task.id.clone())
        .collect()
}

/// Resolves an active v2 policy for one instance (Workflow #335 S2b): each target's pool, its
/// step and importance, its named tasks and its inventory once, then the resource term of
/// every catalog task whose scope covers the instance. The error names a state only a bug
/// reaches.
fn resolve_v2(
    context: &TargetContext<'_>,
    targets: &[ResourceTargetSpecV2],
) -> Result<ResolvedV2, &'static str> {
    use ResourceTargetsRejectionReason as Reason;
    let TargetContext {
        catalog,
        projected_facts,
        instance,
        time,
        ..
    } = *context;
    let mut resolved = Vec::with_capacity(targets.len());
    let mut unmapped = BTreeMap::<String, (String, String, String, &'static str)>::new();
    let mut unevaluable = Vec::new();
    for target in targets {
        let mut mark_unmapped = |task: &str, why| {
            unmapped.insert(
                task.to_owned(),
                (
                    target.id.clone(),
                    target.resource.clone(),
                    task.to_owned(),
                    why,
                ),
            );
        };
        let mut lapsed = |why: &str| {
            unevaluable.push(format!("{}@{}:{why}", target.id, target.resource));
        };
        let (pool, fact_key) = match resolve_target_pool(catalog, &target.resource, instance) {
            Ok(found) => found,
            Err((reason, _)) => {
                let why = resolution_why(reason)?;
                if target.tasks.is_none() {
                    lapsed(why);
                }
                for task in target.tasks.iter().flatten() {
                    match target_task(catalog, task, instance) {
                        Ok(_) => mark_unmapped(task, why),
                        Err(reason) => unevaluable.push(format!(
                            "{}/{task}:{}",
                            target.id,
                            resolution_why(reason)?
                        )),
                    }
                }
                continue;
            }
        };
        // A target without `tasks` covers the producing candidates; when no task of the
        // instance produces its pool in the active catalog it can weigh nothing, and says so.
        // A producing task only gated this round is not a lapse.
        if target.tasks.is_none()
            && !catalog.catalog().tasks.tasks.iter().any(|task| {
                task_on_instance(task, instance).is_ok() && per_run_production(task, &pool.id) >= 1
            })
        {
            lapsed("no_producing_task");
            continue;
        }
        let named = match &target.tasks {
            None => None,
            Some(tasks) => {
                let mut named = BTreeSet::new();
                for task in tasks {
                    match resolve_target_task(catalog, pool, task, instance) {
                        Ok(_) => {
                            named.insert(task.as_str());
                        }
                        Err(Reason::UnmappedTask) => mark_unmapped(task, "task_not_producing"),
                        Err(reason) => unevaluable.push(format!(
                            "{}/{task}:{}",
                            target.id,
                            resolution_why(reason)?
                        )),
                    }
                }
                Some(named)
            }
        };
        let status = match target_terms(target, pool) {
            Err(_) => TargetStatus::Unresolved {
                why: if pool.valuation.is_none() {
                    "valuation_missing"
                } else {
                    "gap_missing"
                },
            },
            Ok((step, importance_milli)) => {
                match observe_target(projected_facts, instance, pool, &fact_key, time) {
                    TargetObservation::Pending { reason } => TargetStatus::Pending { reason },
                    TargetObservation::Known {
                        current,
                        fresh_until_unix_ms,
                        ..
                    } => {
                        let TargetCondition::AtLeast { amount } = target.condition;
                        let gap = amount.saturating_sub(current.unsigned_abs());
                        if gap == 0 {
                            TargetStatus::Satisfied { current }
                        } else {
                            let (gap_weight_milli, capped) =
                                task_target_milli(gap, importance_milli, step, 1, 1)
                                    .ok_or("resource target gap step is zero")?;
                            TargetStatus::Applied {
                                current,
                                gap,
                                step,
                                importance_milli,
                                gap_weight_milli,
                                capped,
                                inventory_expires_at_unix_ms: fresh_until_unix_ms,
                            }
                        }
                    }
                }
            }
        };
        resolved.push(ResolvedTargetV2 {
            target,
            pool,
            named,
            status,
        });
    }
    let empty = ResourceTerm {
        term_milli: 0,
        capped: false,
        override_target: None,
        valid_until_unix_ms: context.valid_until_unix_ms,
        inventory_expiries: Vec::new(),
        policy_sha256: context.policy_sha256.to_owned(),
        applied_at_unix_ms: context.applied_at_unix_ms,
        targets: Vec::new(),
        unmapped: None,
        items: Vec::new(),
    };
    let pools = catalog
        .catalog()
        .pools
        .pools
        .iter()
        .filter(|pool| scope_matches_instance(&pool.scope, instance))
        .collect::<Vec<_>>();
    let mut terms = BTreeMap::new();
    for task in catalog
        .catalog()
        .tasks
        .tasks
        .iter()
        .filter(|task| scope_matches_instance(&task.scope, instance))
    {
        let mut term = ResourceTerm {
            unmapped: unmapped.get(&task.id).cloned(),
            ..empty.clone()
        };
        for target in &resolved {
            if !target.covers(&task.id, per_run_production(task, &target.pool.id)) {
                continue;
            }
            term.targets.push(target.item());
            let overriding = target.target.apply.mode == TargetMode::Override
                && matches!(target.status, TargetStatus::Applied { .. });
            if overriding {
                term.override_target = Some((
                    target.target.id.clone(),
                    target
                        .target
                        .apply
                        .manual_offset
                        .unwrap_or(ManualOffset::Keep),
                ));
            }
            if let TargetStatus::Applied {
                gap_weight_milli,
                inventory_expires_at_unix_ms: Some(expiry),
                ..
            } = target.status
                && (gap_weight_milli > 0 || overriding)
            {
                term.inventory_expiries.push(expiry);
            }
        }
        let mut total = 0_u128;
        let mut fraction = (0_u128, 1_u128);
        for pool in &pools {
            let per_run = per_run_production(task, &pool.id);
            if per_run == 0 {
                continue;
            }
            let covering = resolved
                .iter()
                .find(|target| target.pool.id == pool.id && target.covers(&task.id, per_run));
            let valuation = pool.valuation.as_ref();
            let Some(step) = valuation
                .map(|valuation| valuation.scale)
                .or_else(|| covering.and_then(|target| target.target.scale))
            else {
                continue;
            };
            let base_milli =
                valuation.map_or(0, |valuation| u64::from(valuation.base_weight_milli));
            let gap_weight_milli = covering.map_or(0, ResolvedTargetV2::gap_weight_milli);
            let effective_milli = base_milli + gap_weight_milli;
            let numerator = per_run
                .checked_mul(u128::from(effective_milli))
                .ok_or("expected production weighted amount overflow")?;
            let denominator = u128::from(step) * 1_000;
            let term_milli = numerator
                .checked_div(denominator)
                .ok_or("resource valuation step is zero")?;
            total = total
                .checked_add(term_milli)
                .ok_or("resource score sum overflow")?;
            if task
                .produces
                .iter()
                .any(|effect| effect.pool_id == pool.id && effect.expected_amount_milli.is_some())
            {
                add_score_fraction(&mut fraction, numerator % denominator, denominator)?;
            }
            term.items.push(WeightItem {
                pool: pool.id.clone(),
                per_run,
                step,
                base_milli,
                gap_weight_milli,
                effective_milli,
                term_milli,
            });
        }
        term.items.sort_by(|left, right| {
            right
                .term_milli
                .cmp(&left.term_milli)
                .then_with(|| left.pool.cmp(&right.pool))
        });
        // Integer declarations retain their per-resource floor. Explicit expected
        // contributions carry their exact remainder to this final score boundary.
        total = total
            .checked_add(fraction.0 / fraction.1)
            .ok_or("resource score sum overflow")?;
        let cap = u128::from(MAX_RESOURCE_TARGET_MILLI);
        term.capped = total > cap;
        term.term_milli =
            u64::try_from(total.min(cap)).map_err(|_| "resource term cap exceeds u64")?;
        terms.insert(task.id.clone(), term);
    }
    Ok(ResolvedV2 {
        terms,
        unevaluable,
        empty,
    })
}

/// Resolves one active target (T1) and records the effect of each task it names.
fn target_effects(
    context: &TargetContext<'_>,
    target: &ResourceTargetSpec,
    effects: &mut BTreeMap<String, TargetEffect>,
    unevaluable: &mut Vec<(String, String, &'static str)>,
) -> Result<(), &'static str> {
    use ResourceTargetsRejectionReason as Reason;
    let TargetContext {
        catalog,
        projected_facts,
        instance,
        time,
        ..
    } = *context;
    let unmapped = |task: &str, why| TargetEffect::Unmapped {
        id: target.id.clone(),
        resource: target.resource.clone(),
        task: task.to_owned(),
        why,
    };
    let (pool, fact_key) = match resolve_target_pool(catalog, &target.resource, instance) {
        Ok(resolved) => resolved,
        Err((reason, _)) => {
            let why = resolution_why(reason)?;
            for task in &target.tasks {
                match target_task(catalog, task, instance) {
                    Ok(_) => {
                        effects.insert(task.clone(), unmapped(task, why));
                    }
                    Err(reason) => {
                        unevaluable.push((target.id.clone(), task.clone(), resolution_why(reason)?))
                    }
                }
            }
            return Ok(());
        }
    };
    let mut mapped = Vec::with_capacity(target.tasks.len());
    for task in &target.tasks {
        match resolve_target_task(catalog, pool, task, instance) {
            Ok(per_run) => mapped.push((task, per_run)),
            Err(Reason::UnmappedTask) => {
                effects.insert(task.clone(), unmapped(task, "task_not_producing"));
            }
            Err(reason) => {
                unevaluable.push((target.id.clone(), task.clone(), resolution_why(reason)?));
            }
        }
    }
    if mapped.is_empty() {
        return Ok(());
    }
    let TargetCondition::AtLeast { amount: at_least } = target.condition;
    let (current, inventory_expires_at_unix_ms) =
        match observe_target(projected_facts, instance, pool, &fact_key, time) {
            TargetObservation::Pending { reason } => {
                for (task, _) in mapped {
                    effects.insert(
                        task.clone(),
                        TargetEffect::Pending {
                            id: target.id.clone(),
                            fact_key: fact_key.clone(),
                            reason,
                        },
                    );
                }
                return Ok(());
            }
            TargetObservation::Known {
                current,
                fresh_until_unix_ms,
                ..
            } => (current, fresh_until_unix_ms),
        };
    let gap = at_least.saturating_sub(current.unsigned_abs());
    if gap == 0 {
        for (task, _) in mapped {
            effects.insert(
                task.clone(),
                TargetEffect::Satisfied {
                    id: target.id.clone(),
                    policy_sha256: context.policy_sha256.to_owned(),
                    applied_at_unix_ms: context.applied_at_unix_ms,
                    current,
                    at_least,
                    mode: target.apply.mode,
                },
            );
        }
        return Ok(());
    }
    const NO_BEST: &str = "resource target task score has a zero divisor or arithmetic overflow";
    let gap_milli = u128::from(gap) * 1_000;
    let best_useful = mapped
        .iter()
        .map(|(_, per_run)| (*per_run).min(gap_milli))
        .max()
        .unwrap_or(0);
    let (weight_milli, _) =
        task_target_milli(gap, target.importance_milli, target.scale, 1, 1).ok_or(NO_BEST)?;
    for (task, per_run) in mapped {
        let useful = per_run.min(gap_milli);
        let (task_target_milli, capped) = task_target_milli(
            gap,
            target.importance_milli,
            target.scale,
            useful,
            best_useful,
        )
        .ok_or(NO_BEST)?;
        effects.insert(
            task.clone(),
            TargetEffect::Applied(Box::new(AppliedTarget {
                id: target.id.clone(),
                mode: target.apply.mode,
                task_target_milli,
                valid_until_unix_ms: context.valid_until_unix_ms,
                inventory_expires_at_unix_ms,
                capped,
                policy_sha256: context.policy_sha256.to_owned(),
                applied_at_unix_ms: context.applied_at_unix_ms,
                resource: pool.id.clone(),
                fact_key: fact_key.clone(),
                current,
                at_least,
                gap,
                scale: target.scale,
                importance_milli: target.importance_milli,
                weight_milli,
                per_run,
                useful,
                best_useful,
            })),
        );
    }
    Ok(())
}

impl InstanceTargets {
    /// The effect `task_id`'s candidate meets on this instance.
    pub(crate) fn effect(&self, task_id: &str) -> TargetEffect {
        match &self.policy {
            ResolvedPolicy::None => TargetEffect::None,
            ResolvedPolicy::Unreadable(code) => TargetEffect::PolicyUnreadable { code },
            ResolvedPolicy::Expired {
                policy_sha256,
                applied_at_unix_ms,
                valid_until_unix_ms,
                named,
            } if named.contains(task_id) => TargetEffect::PolicyExpired {
                policy_sha256: policy_sha256.clone(),
                applied_at_unix_ms: *applied_at_unix_ms,
                valid_until_unix_ms: *valid_until_unix_ms,
            },
            ResolvedPolicy::Expired { .. } => TargetEffect::None,
            ResolvedPolicy::Active { effects, .. } => {
                effects.get(task_id).cloned().unwrap_or(TargetEffect::None)
            }
            ResolvedPolicy::ActiveV2(resolved) => TargetEffect::Resources(Box::new(
                resolved
                    .terms
                    .get(task_id)
                    .unwrap_or(&resolved.empty)
                    .clone(),
            )),
        }
    }

    /// A candidate's resource target reasons, at most three, in this order: the policy-level
    /// one (the first of ignored, unreadable, expired and tasks_unevaluable that applies), the
    /// target-level one (applied, satisfied, pending or unmapped) and, for an applied override,
    /// the selection score and manual offset it supersedes. Under an active v2 policy, at most
    /// five: the policy-level one, then those of [`ResourceTerm`].
    pub(crate) fn reasons(
        &self,
        effect: &TargetEffect,
        score_milli: Option<i64>,
        offset_milli: i64,
    ) -> Vec<DecisionReason> {
        let policy_reason = if let Some((scope_kind, observed_at_unix_ms)) = self.ignored {
            Some(DecisionReason {
                code: "resource_target_policy_ignored".to_owned(),
                detail: format!(
                    "a {scope_kind} scoped {RESOURCE_TARGETS_FACT_KEY} record (observed_at={observed_at_unix_ms}) was not written by the formal entry and is ignored"
                ),
            })
        } else {
            match (effect, &self.policy) {
                (TargetEffect::PolicyUnreadable { code }, _) => Some(DecisionReason {
                    code: "resource_target_policy_unreadable".to_owned(),
                    detail: format!(
                        "stored policy cannot be read ({code}); instance runs base scheduling"
                    ),
                }),
                (
                    TargetEffect::PolicyExpired {
                        policy_sha256,
                        applied_at_unix_ms,
                        valid_until_unix_ms,
                    },
                    _,
                ) => Some(DecisionReason {
                    code: "resource_target_policy_expired".to_owned(),
                    detail: format!(
                        "policy={policy_sha256} applied_at={applied_at_unix_ms} valid_until={valid_until_unix_ms}"
                    ),
                }),
                (_, ResolvedPolicy::Active { unevaluable, .. }) if !unevaluable.is_empty() => {
                    Some(DecisionReason {
                        code: "resource_target_tasks_unevaluable".to_owned(),
                        detail: unevaluable_detail(unevaluable),
                    })
                }
                (_, ResolvedPolicy::ActiveV2(resolved)) if !resolved.unevaluable.is_empty() => {
                    Some(DecisionReason {
                        code: "resource_target_tasks_unevaluable".to_owned(),
                        detail: bounded_list(String::new(), &resolved.unevaluable, ","),
                    })
                }
                _ => None,
            }
        };
        let mut reasons = Vec::with_capacity(3);
        reasons.extend(policy_reason);
        if let TargetEffect::Resources(term) = effect {
            reasons.extend(term.reasons(score_milli, offset_milli));
            return reasons;
        }
        reasons.extend(effect.target_reason());
        if let Some(applied) = effect.applied()
            && applied.mode == TargetMode::Override
        {
            reasons.push(DecisionReason {
                code: format!("resource_target_override:{}", applied.id),
                detail: format!(
                    "superseded score={} offset={offset_milli}; utility kept",
                    score_milli.map_or_else(|| "none".to_owned(), |score| score.to_string())
                ),
            });
        }
        reasons
    }
}

/// `<target>/<task>:<why>` items joined by commas in target and task order, counted while
/// joining so the items take at most 1010 bytes; the rest is folded into `+<n>more`.
fn unevaluable_detail(items: &[(String, String, &'static str)]) -> String {
    const ITEM_BUDGET_BYTES: usize = 1_010;
    let mut detail = String::new();
    for (index, (target, task, why)) in items.iter().enumerate() {
        let item = format!("{target}/{task}:{why}");
        let separator = usize::from(!detail.is_empty());
        if detail.len() + separator + item.len() > ITEM_BUDGET_BYTES {
            if separator == 1 {
                detail.push(',');
            }
            detail.push_str(&format!("+{}more", items.len() - index));
            break;
        }
        if separator == 1 {
            detail.push(',');
        }
        detail.push_str(&item);
    }
    detail
}

/// The `why` of an unresolvable target or task; any other reason is a resolver bug.
fn resolution_why(reason: ResourceTargetsRejectionReason) -> Result<&'static str, &'static str> {
    use ResourceTargetsRejectionReason as Reason;
    Ok(match reason {
        Reason::UnknownResource => "unknown_resource",
        Reason::ResourceNotObservable => "resource_not_observable",
        Reason::ResourceOutOfScope => "resource_out_of_scope",
        Reason::UnknownTask => "unknown_task",
        Reason::TaskOutOfScope => "task_out_of_scope",
        Reason::TaskDisabled => "task_disabled",
        Reason::UnmappedTask => "task_not_producing",
        _ => return Err("resource target resolution returned an unexpected reason"),
    })
}

const fn pending_name(reason: ResourceTargetPendingReason) -> &'static str {
    match reason {
        ResourceTargetPendingReason::Missing => "missing",
        ResourceTargetPendingReason::Expired => "expired",
        ResourceTargetPendingReason::LowConfidence => "low_confidence",
        ResourceTargetPendingReason::InvalidValue => "invalid_value",
    }
}

const fn rule_name(rule: TargetRule) -> &'static str {
    match rule {
        TargetRule::ShortfallLinear => "shortfall_linear",
    }
}

pub(crate) const fn mode_name(mode: TargetMode) -> &'static str {
    match mode {
        TargetMode::Adjust => "adjust",
        TargetMode::Override => "override",
    }
}

pub(crate) const fn manual_offset_name(manual_offset: ManualOffset) -> &'static str {
    match manual_offset {
        ManualOffset::Keep => "keep",
        ManualOffset::Supersede => "supersede",
    }
}

const fn weight_name(weight: TargetWeight) -> &'static str {
    match weight {
        TargetWeight::ScoreStage => "score_stage",
    }
}

/// The scheduling identifier charset `^[a-z0-9][a-z0-9._:-]*$`, 1 to 128 bytes.
fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_ID_BYTES
        && value.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || (index > 0 && matches!(byte, b'.' | b'_' | b':' | b'-'))
        })
}

fn rejected_at(
    map: &SourceMap,
    path: String,
    reason: ResourceTargetsRejectionReason,
) -> ResourceTargetsError {
    let located = map.diagnostic(CatalogDiagnosticCode::TypeMismatch, path, "", None);
    rejected(
        located.json_path,
        located.source.line,
        located.source.column,
        reason,
    )
}

/// A rejection whose path is a bounded, control-free JSON pointer (a key may hold escaped
/// control characters; they are shown as U+FFFD) and whose position is at least 1:1.
fn rejected(
    path: String,
    line: u32,
    column: u32,
    reason: ResourceTargetsRejectionReason,
) -> ResourceTargetsError {
    let mut field_path = String::new();
    for character in path.chars() {
        let character = if character.is_control() {
            '\u{fffd}'
        } else {
            character
        };
        if field_path.len() + character.len_utf8() > MAX_REJECTION_FIELD_PATH_BYTES {
            break;
        }
        field_path.push(character);
    }
    ResourceTargetsError::Rejected(Box::new(ResourceTargetsRejection {
        field_path,
        line: line.max(1),
        column: column.max(1),
        reason,
    }))
}
