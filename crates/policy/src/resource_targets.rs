// SPDX-License-Identifier: AGPL-3.0-only

//! Instance resource target policies (Workflow #308 RT-S1a/S1b): the
//! `actingcommand.resource-targets.v1` document, its formal parse and its check against the
//! active catalog and an authoritative projection, the record-list rows the Runtime stores as
//! the instance fact `session.resource_targets`, the pure target helpers the entry and the
//! evaluator share, and the evaluator's reading and scoring of stored policies. Nothing here
//! reads a clock, a file or the network.

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
    CatalogDiagnosticCode, CompiledCatalog, DecisionReason, EvaluationFacts, EvaluationTime,
    FactScalar, FactValue, InstanceSnapshot, MAX_ID_BYTES, ObservationRef, ObservedFact,
    PolicyEvaluationError, PolicyEvaluationResult, PoolSpec, PoolValueSource,
    SchedulingDocumentKind, ScopeSelector, TaskSpec,
};

pub const RESOURCE_TARGETS_SCHEMA_VERSION: &str = "actingcommand.resource-targets.v1";
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

/// A structurally valid document with its source positions.
#[derive(Debug)]
pub struct ParsedResourceTargets {
    document: ResourceTargetsDocument,
    source_map: SourceMap,
}

impl ParsedResourceTargets {
    pub fn document(&self) -> &ResourceTargetsDocument {
        &self.document
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
/// Takes no lock and reads nothing but `bytes`.
pub fn parse_resource_targets(bytes: &[u8]) -> Result<ParsedResourceTargets, ResourceTargetsError> {
    use ResourceTargetsRejectionReason as Reason;
    if bytes.len() > MAX_RESOURCE_TARGETS_DOCUMENT_BYTES {
        return Err(rejected(String::new(), 1, 1, Reason::OutOfRange));
    }
    let source = CatalogDocumentSource::new(DOCUMENT_SOURCE_URI, bytes.to_vec());
    let parsed =
        parse_document::<ResourceTargetsDocument>(&source, SchedulingDocumentKind::ResourceTargets)
            .map_err(|diagnostic| {
                let reason = match diagnostic.code {
                    CatalogDiagnosticCode::InvalidJson => Reason::InvalidJson,
                    CatalogDiagnosticCode::DuplicateKey => Reason::DuplicateKey,
                    CatalogDiagnosticCode::UnknownField => Reason::UnknownField,
                    CatalogDiagnosticCode::MissingRequiredField => Reason::MissingField,
                    CatalogDiagnosticCode::TypeMismatch => Reason::InvalidType,
                    _ => {
                        return ResourceTargetsError::Internal(
                            "resource_targets_parser_code_unexpected",
                        );
                    }
                };
                rejected(
                    diagnostic.json_path,
                    diagnostic.source.line,
                    diagnostic.source.column,
                    reason,
                )
            })?;
    let parsed = ParsedResourceTargets {
        document: parsed.value,
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

/// One target resolved against a catalog for one instance: its pool, the pool's inventory
/// fact key and, per named task, the expected effective production of one run (`r_k`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedTarget<'a> {
    pub(crate) pool: &'a PoolSpec,
    pub(crate) fact_key: String,
    pub(crate) per_task: Vec<(String, u64)>,
}

/// Resolves `target` for `instance`: its pool, then each task, the first refusal winning.
/// The entry check and the evaluator share both halves, so both judge a target alike. A
/// refusal names its reason and its path below the target.
pub(crate) fn resolve_target<'a>(
    catalog: &'a CompiledCatalog,
    target: &ResourceTargetSpec,
    instance: &InstanceSnapshot,
) -> Result<ResolvedTarget<'a>, (ResourceTargetsRejectionReason, String)> {
    let (pool, fact_key) = resolve_target_pool(catalog, target, instance)?;
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

/// The pool half of [`resolve_target`]: the target's pool and its inventory fact key, or the
/// refusal at `/resource`.
pub(crate) fn resolve_target_pool<'a>(
    catalog: &'a CompiledCatalog,
    target: &ResourceTargetSpec,
    instance: &InstanceSnapshot,
) -> Result<(&'a PoolSpec, String), (ResourceTargetsRejectionReason, String)> {
    use ResourceTargetsRejectionReason as Reason;
    let pool = catalog
        .catalog()
        .pools
        .pools
        .iter()
        .find(|pool| pool.id == target.resource)
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
/// (`r_k >= 1`) by `task_id` on `instance`, or why the task cannot serve the target.
pub(crate) fn resolve_target_task(
    catalog: &CompiledCatalog,
    pool: &PoolSpec,
    task_id: &str,
    instance: &InstanceSnapshot,
) -> Result<u64, ResourceTargetsRejectionReason> {
    let task = target_task(catalog, task_id, instance)?;
    let produced = task
        .produces
        .iter()
        .filter(|effect| effect.pool_id == pool.id)
        .map(|effect| u128::from(effect.amount) * u128::from(effect.confidence_milli) / 1_000)
        .sum::<u128>();
    let per_run = u64::try_from(produced).unwrap_or(u64::MAX);
    if per_run == 0 {
        return Err(ResourceTargetsRejectionReason::UnmappedTask);
    }
    Ok(per_run)
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
    Ok(task)
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

/// T1 task target score `min(⌊g·I·u / (S·U)⌋, 1_000_000)` in exact integer arithmetic, and
/// whether the cap applied; `None` when `S·U` is zero (never for a checked target with a gap).
pub(crate) fn task_target_milli(
    gap: u64,
    importance_milli: u64,
    scale: u64,
    useful: u64,
    best_useful: u64,
) -> Option<(u64, bool)> {
    let numerator = u128::from(gap) * u128::from(importance_milli) * u128::from(useful);
    let quotient = numerator.checked_div(u128::from(scale) * u128::from(best_useful))?;
    let cap = u128::from(MAX_RESOURCE_TARGET_MILLI);
    Some(if quotient > cap {
        (MAX_RESOURCE_TARGET_MILLI, true)
    } else {
        (u64::try_from(quotient).ok()?, false)
    })
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
    per_run: u64,
    useful: u64,
    best_useful: u64,
}

impl TargetEffect {
    pub(crate) fn applied(&self) -> Option<&AppliedTarget> {
        match self {
            Self::Applied(applied) => Some(applied),
            _ => None,
        }
    }

    /// The target-level reason: applied, satisfied, pending or unmapped.
    fn target_reason(&self) -> Option<DecisionReason> {
        let (code, detail) = match self {
            Self::None | Self::PolicyUnreadable { .. } | Self::PolicyExpired { .. } => return None,
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
                    applied.per_run,
                    applied.useful,
                    applied.best_useful,
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
    };
    Ok(InstanceTargets {
        ignored: policy.ignored,
        policy: resolved,
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
    let (pool, fact_key) = match resolve_target_pool(catalog, target, instance) {
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
    const NO_BEST: &str = "resource target task score has no best useful contribution";
    let best_useful = mapped
        .iter()
        .map(|(_, per_run)| (*per_run).min(gap))
        .max()
        .unwrap_or(0);
    let (weight_milli, _) =
        task_target_milli(gap, target.importance_milli, target.scale, 1, 1).ok_or(NO_BEST)?;
    for (task, per_run) in mapped {
        let useful = per_run.min(gap);
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
        }
    }

    /// A candidate's resource target reasons, at most three, in this order: the policy-level
    /// one (the first of ignored, unreadable, expired and tasks_unevaluable that applies), the
    /// target-level one (applied, satisfied, pending or unmapped) and, for an applied override,
    /// the selection score and manual offset it supersedes.
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
                _ => None,
            }
        };
        let mut reasons = Vec::with_capacity(3);
        reasons.extend(policy_reason);
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
