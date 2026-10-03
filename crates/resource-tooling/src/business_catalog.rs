// SPDX-License-Identifier: AGPL-3.0-only

//! Static authoring catalog -> production identity declarations and inline lookup transforms.
//! No network, execution, inventory facts, or Runtime catalog parser.

use actingcommand_contract::{LabError, LabResult};
use actingcommand_recognition_pack::{
    CandidateIdentityDeclaration, CandidateIdentityEntry, CandidateIdentityRecognition,
};
use actingcommand_selection_policy::{
    FieldDeclaration, LookupEntry, LookupKey, Transform, ValueType,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub const BUSINESS_CATALOG_SCHEMA: &str = "actingcommand.business-catalog.v1";
pub const MAX_BUSINESS_CATALOG_BYTES: usize = 1024 * 1024;
const MAX_INTEGER: u64 = 9_007_199_254_740_991;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BusinessCatalog {
    pub schema_version: String,
    pub catalog_id: String,
    pub recognition: CandidateIdentityRecognition,
    /// The external pool IDs and capacities against which generated effects close.
    pub pools: BTreeMap<String, u64>,
    pub entries: Vec<BusinessCatalogEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogSource {
    pub uri: String,
    pub date: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BusinessCatalogEntry {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    /// Server -> official name followed by aliases. Only the selected server is compiled.
    pub names: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub duration_seconds: Option<u64>,
    #[serde(default)]
    pub costs: Vec<CatalogCost>,
    #[serde(default)]
    pub rewards: Vec<CatalogReward>,
    #[serde(default)]
    pub preference_milli: Option<i64>,
    pub source: CatalogSource,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogCost {
    pub pool_id: String,
    pub amount: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogReward {
    pub pool_id: String,
    /// Real quantity in milli-units per successful batch; independent of probability.
    pub quantity_milli: Option<u64>,
    pub probability_milli: Option<u16>,
    pub batches: Option<u32>,
    /// Source evidence confidence, never multiplied into expected quantity.
    pub confidence_milli: Option<u16>,
    pub observation_source: String,
    /// Historical observation kept only as source material. Never emitted as inventory.
    #[serde(default)]
    pub observed_amount: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogUnknown {
    pub id: String,
    pub field: String,
    pub reason: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogExpectedProduction {
    pub pool_id: String,
    pub direction: &'static str,
    pub expected_amount_milli: u64,
    pub observation_source: String,
    pub confidence_milli: u16,
}

#[derive(Debug, Clone, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CompiledBusinessCatalog {
    pub schema_version: &'static str,
    pub source_sha256: String,
    pub catalog_id: String,
    pub server: String,
    pub identity_feature: serde_json::Value,
    pub policy_field: FieldDeclaration,
    /// Existing selection-policy Transform values. Missing keys remain lookup_miss.
    pub lookup_transforms: BTreeMap<String, Transform>,
    pub scheduling_schema: &'static str,
    pub scheduling_produces: BTreeMap<String, Vec<CatalogExpectedProduction>>,
    pub unknown_values: Vec<CatalogUnknown>,
    pub provenance: BusinessCatalog,
}

/// Deterministic for the exact input bytes, selected server and field. The caller may store
/// the returned authoring output; Runtime consumes only the generated formal declarations.
pub fn compile_business_catalog(
    bytes: &[u8],
    server: &str,
    field: &str,
) -> LabResult<CompiledBusinessCatalog> {
    if bytes.len() > MAX_BUSINESS_CATALOG_BYTES {
        return Err(invalid("catalog exceeds 1 MiB"));
    }
    let mut catalog: BusinessCatalog =
        serde_json::from_slice(bytes).map_err(|error| invalid(error.to_string()))?;
    actingcommand_contract::validate_candidate_feature_name(field)
        .map_err(|error| invalid(error.to_string()))?;
    if catalog.schema_version != BUSINESS_CATALOG_SCHEMA
        || !token(&catalog.catalog_id)
        || !token(server)
        || !(1..=128).contains(&catalog.entries.len())
        || catalog.pools.len() > 128
        || catalog
            .pools
            .iter()
            .any(|(id, capacity)| !token(id) || *capacity == 0 || *capacity > MAX_INTEGER)
    {
        return Err(invalid(
            "catalog schema, identity, row count or pool capacity is invalid",
        ));
    }
    catalog.entries.sort_by(|a, b| a.id.cmp(&b.id));
    let mut ids = BTreeSet::new();
    let mut identity_entries = Vec::new();
    let mut lookup_rows = BTreeMap::<String, Vec<LookupEntry>>::new();
    let mut produces = BTreeMap::new();
    let mut unknown = Vec::new();
    for entry in &mut catalog.entries {
        if !token(&entry.id)
            || !ids.insert(entry.id.clone())
            || entry
                .variant
                .as_ref()
                .is_some_and(|variant| !token(variant))
            || entry.names.len() > 32
            || entry.names.keys().any(|server| !token(server))
            || entry.source.uri.is_empty()
            || entry.source.uri.len() > 2048
            || entry.source.uri.chars().any(char::is_control)
            || !valid_date(&entry.source.date)
            || entry.costs.len() > 128
            || entry.rewards.len() > 128
            || entry
                .duration_seconds
                .is_some_and(|value| value > MAX_INTEGER)
            || entry
                .preference_milli
                .is_some_and(|value| value.unsigned_abs() > MAX_INTEGER)
        {
            return Err(invalid(format!(
                "catalog entry '{}' violates a bound or duplicates a stable ID",
                entry.id
            )));
        }
        for aliases in entry.names.values_mut() {
            if aliases.is_empty()
                || aliases.len() > 8
                || aliases.iter().any(|alias| {
                    alias.is_empty()
                        || alias.len() > 512
                        || alias.chars().count() > 128
                        || alias.chars().any(char::is_control)
                })
            {
                return Err(invalid(format!(
                    "catalog entry '{}' has invalid names or aliases",
                    entry.id
                )));
            }
            aliases.sort();
            aliases.dedup();
        }
        let aliases = entry
            .names
            .get(server)
            .ok_or_else(|| {
                invalid(format!(
                    "catalog entry '{}' has no names for server '{server}'",
                    entry.id
                ))
            })?
            .clone();
        identity_entries.push(CandidateIdentityEntry {
            id: entry.id.clone(),
            variant: entry.variant.clone(),
            aliases,
        });
        let mut lookup = |name: String, value: Option<i64>| {
            if let Some(value_milli) = value {
                lookup_rows.entry(name).or_default().push(LookupEntry {
                    key: LookupKey::String(entry.id.clone()),
                    value_milli,
                });
            } else {
                unknown.push(CatalogUnknown {
                    id: entry.id.clone(),
                    field: name,
                    reason: "not_confirmed",
                });
            }
        };
        lookup(
            "duration_seconds".into(),
            entry.duration_seconds.map(as_milli).transpose()?,
        );
        lookup("preference_milli".into(), entry.preference_milli);
        entry.costs.sort_by(|a, b| a.pool_id.cmp(&b.pool_id));
        let mut costs = BTreeSet::new();
        for cost in &entry.costs {
            let capacity = catalog
                .pools
                .get(&cost.pool_id)
                .ok_or_else(|| invalid(format!("undeclared cost pool '{}'", cost.pool_id)))?;
            if !costs.insert(&cost.pool_id)
                || cost
                    .amount
                    .is_some_and(|amount| amount > *capacity || amount > MAX_INTEGER)
            {
                return Err(invalid(format!("invalid cost for '{}'", entry.id)));
            }
            lookup(
                format!("cost:{}", cost.pool_id),
                cost.amount.map(as_milli).transpose()?,
            );
        }
        entry.rewards.sort_by(|a, b| a.pool_id.cmp(&b.pool_id));
        let mut rewards = BTreeSet::new();
        let mut row_produces = Vec::new();
        for reward in &entry.rewards {
            let capacity = catalog
                .pools
                .get(&reward.pool_id)
                .ok_or_else(|| invalid(format!("undeclared reward pool '{}'", reward.pool_id)))?;
            if !rewards.insert(&reward.pool_id)
                || reward
                    .quantity_milli
                    .is_some_and(|value| value > MAX_INTEGER)
                || reward.probability_milli.is_some_and(|value| value > 1000)
                || reward.confidence_milli.is_some_and(|value| value > 1000)
                || reward
                    .batches
                    .is_some_and(|value| !(1..=1_000_000).contains(&value))
                || reward
                    .observed_amount
                    .is_some_and(|value| value > MAX_INTEGER)
                || !matches!(
                    reward.observation_source.as_str(),
                    "self_reported" | "scan_verified" | "inferred"
                )
            {
                return Err(invalid(format!(
                    "invalid reward dimensions for '{}'",
                    entry.id
                )));
            }
            lookup(
                format!("probability_milli:{}", reward.pool_id),
                reward.probability_milli.map(i64::from),
            );
            lookup(
                format!("evidence_confidence_milli:{}", reward.pool_id),
                reward.confidence_milli.map(i64::from),
            );
            let expected = match (
                reward.quantity_milli,
                reward.probability_milli,
                reward.batches,
            ) {
                (Some(quantity), Some(probability), Some(batches)) => {
                    let numerator =
                        u128::from(quantity) * u128::from(probability) * u128::from(batches);
                    if numerator % 1000 != 0 {
                        return Err(invalid(format!(
                            "reward '{}/{}' is not exactly representable in milli-units",
                            entry.id, reward.pool_id
                        )));
                    }
                    let expected = numerator / 1000;
                    if expected > u128::from(MAX_INTEGER) || expected > u128::from(*capacity) * 1000
                    {
                        return Err(invalid(format!(
                            "expected reward '{}/{}' exceeds its numeric or pool capacity bound",
                            entry.id, reward.pool_id
                        )));
                    }
                    Some(expected as u64)
                }
                _ => None,
            };
            lookup(
                format!("expected_reward_milli:{}", reward.pool_id),
                expected.map(|value| value as i64),
            );
            if let (Some(expected_amount_milli), Some(confidence_milli)) =
                (expected, reward.confidence_milli)
            {
                row_produces.push(CatalogExpectedProduction {
                    pool_id: reward.pool_id.clone(),
                    direction: "produce",
                    expected_amount_milli,
                    observation_source: reward.observation_source.clone(),
                    confidence_milli,
                });
            }
        }
        produces.insert(entry.id.clone(), row_produces);
    }
    let identity = CandidateIdentityDeclaration {
        entries: identity_entries,
        recognition: catalog.recognition.clone(),
    };
    identity.validate().map_err(invalid)?;
    let policy_field = FieldDeclaration {
        name: field.to_owned(),
        value_type: ValueType::EnumString {
            allowed: identity.domain(),
        },
    };
    let compiled = CompiledBusinessCatalog {
        schema_version: "actingcommand.compiled-business-catalog.v1",
        source_sha256: format!("{:x}", Sha256::digest(bytes)),
        catalog_id: catalog.catalog_id.clone(),
        server: server.to_owned(),
        identity_feature: serde_json::json!({"name":field,"value":"identity","identity":identity}),
        policy_field,
        lookup_transforms: lookup_rows
            .into_iter()
            .map(|(name, entries)| {
                (
                    name,
                    Transform::Lookup {
                        entries,
                        default_milli: None,
                    },
                )
            })
            .collect(),
        scheduling_schema: "actingcommand.scheduling.v2",
        scheduling_produces: produces,
        unknown_values: unknown,
        provenance: catalog,
    };
    if serde_json::to_vec(&compiled)
        .map_err(|error| invalid(error.to_string()))?
        .len()
        > MAX_BUSINESS_CATALOG_BYTES * 2
    {
        return Err(invalid("compiled catalog exceeds 2 MiB"));
    }
    Ok(compiled)
}

fn token(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128 && !value.chars().any(char::is_control)
}

fn as_milli(value: u64) -> LabResult<i64> {
    value
        .checked_mul(1000)
        .filter(|value| *value <= MAX_INTEGER)
        .map(|value| value as i64)
        .ok_or_else(|| invalid("catalog lookup exceeds the exact integer milli range"))
}

fn valid_date(date: &str) -> bool {
    let bytes = date.as_bytes();
    if bytes.len() != 10
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes
            .iter()
            .enumerate()
            .any(|(index, byte)| index != 4 && index != 7 && !byte.is_ascii_digit())
    {
        return false;
    }
    let year = date[..4].parse::<u32>().unwrap_or(0);
    let month = date[5..7].parse::<u32>().unwrap_or(0);
    let day = date[8..].parse::<u32>().unwrap_or(0);
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year.is_multiple_of(400) || (year.is_multiple_of(4) && !year.is_multiple_of(100)) => {
            29
        }
        2 => 28,
        _ => 0,
    };
    year > 0 && day > 0 && day <= days
}

fn invalid(message: impl Into<String>) -> LabError {
    LabError::package_invalid(message)
}
