// SPDX-License-Identifier: AGPL-3.0-only

//! Instance resource target policies (Workflow #308 RT-S1a): the
//! `actingcommand.resource-targets.v1` document, its formal parse and its check against the
//! active catalog and an authoritative projection, the record-list rows the Runtime stores as
//! the instance fact `session.resource_targets`, and the pure target helpers the evaluator
//! shares. Nothing here reads a clock, a file or the network.

use std::collections::{BTreeMap, BTreeSet};

use actingcommand_contract::{
    FactScalar as ContractFactScalar, MAX_RESOURCE_TARGETS, MAX_RESOURCE_TARGETS_DOCUMENT_BYTES,
    ResourceTargetCondition, ResourceTargetConditionState, ResourceTargetPendingReason,
    ResourceTargetsRejection, ResourceTargetsRejectionReason,
};
use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};

use crate::canonical::canonical_serialized;
use crate::evaluator::{activity_scope_specificity, project_time_validity, scope_matches_instance};
use crate::source::{CatalogDocumentSource, SourceMap, parse_document};
use crate::{
    CatalogDiagnosticCode, CompiledCatalog, EvaluationFacts, EvaluationTime, FactScalar, FactValue,
    InstanceSnapshot, MAX_ID_BYTES, ObservationRef, PolicyEvaluationError, PoolSpec,
    PoolValueSource, SchedulingDocumentKind,
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
                state: match observe_target(&projected, instance, resolved, time) {
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

/// Resolves `target` for `instance`; the entry check and the evaluator share it, so both
/// judge a target alike. A refusal names its reason and its path below the target.
pub(crate) fn resolve_target<'a>(
    catalog: &'a CompiledCatalog,
    target: &ResourceTargetSpec,
    instance: &InstanceSnapshot,
) -> Result<ResolvedTarget<'a>, (ResourceTargetsRejectionReason, String)> {
    use ResourceTargetsRejectionReason as Reason;
    let bundle = catalog.catalog();
    let pool = bundle
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
    let mut per_task = Vec::with_capacity(target.tasks.len());
    for (index, task_id) in target.tasks.iter().enumerate() {
        let path = format!("/tasks/{index}");
        let task = bundle
            .tasks
            .tasks
            .iter()
            .find(|task| task.id == *task_id)
            .ok_or_else(|| (Reason::UnknownTask, path.clone()))?;
        if !scope_matches_instance(&task.scope, instance) {
            return Err((Reason::TaskOutOfScope, path));
        }
        if task.instance_overrides.iter().any(|entry| {
            entry.instance_id == instance.instance_id && entry.enabled.0 == Some(false)
        }) {
            return Err((Reason::TaskDisabled, path));
        }
        let produced = task
            .produces
            .iter()
            .filter(|effect| effect.pool_id == pool.id)
            .map(|effect| u128::from(effect.amount) * u128::from(effect.confidence_milli) / 1_000)
            .sum::<u128>();
        let per_run = u64::try_from(produced).unwrap_or(u64::MAX);
        if per_run == 0 {
            return Err((Reason::UnmappedTask, path));
        }
        per_task.push((task.id.clone(), per_run));
    }
    Ok(ResolvedTarget {
        pool,
        fact_key: fact_key.clone(),
        per_task,
    })
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
    resolved: &ResolvedTarget<'_>,
    time: EvaluationTime,
) -> TargetObservation {
    use ResourceTargetPendingReason as Reason;
    let fact = projected_facts
        .facts
        .iter()
        .filter(|fact| {
            fact.fact_key == resolved.fact_key && scope_matches_instance(&fact.scope, instance)
        })
        .max_by_key(|fact| activity_scope_specificity(&fact.scope));
    let Some(fact) = fact else {
        return TargetObservation::Pending {
            reason: Reason::Missing,
        };
    };
    let minimum_confidence = match resolved.pool.value_source {
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
    #[allow(
        dead_code,
        reason = "the evaluator reads stored policies in Workflow #308 RT-S1b"
    )]
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
/// types, bounded values, unique targets, and task rows that name an earlier target and
/// reference each task once.
#[allow(
    dead_code,
    reason = "the evaluator reads stored policies in Workflow #308 RT-S1b"
)]
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
                if !valid_identifier(&id) || targets.iter().any(|target| target.id == id) {
                    return Err(MalformedRows);
                }
                targets.push(ResourceTargetSpec {
                    id,
                    resource: text(row, "resource")?,
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
                if task.is_empty()
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
#[allow(
    dead_code,
    reason = "the evaluator scores resource targets in Workflow #308 RT-S1b"
)]
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

const fn rule_name(rule: TargetRule) -> &'static str {
    match rule {
        TargetRule::ShortfallLinear => "shortfall_linear",
    }
}

const fn mode_name(mode: TargetMode) -> &'static str {
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
