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
//! sealed periodically as a set of [`RuntimeFactSnapshot`] parts (iron rule 13).

use crate::codes::{SanitizationCode, SharedCode};
use crate::event::{InstanceId, OriginModule, SanitizationError};
use crate::fact::{FactScalar, FactValue};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Schema identity carried by every sealed snapshot.
pub const RUNTIME_FACT_SCHEMA_VERSION: &str = "actingcommand.runtime-fact.v1";
/// Upper bound on live records held by one Runtime process.
pub const MAX_RUNTIME_FACTS: usize = 16_384;
/// Upper bound on one key, in bytes.
pub const MAX_RUNTIME_FACT_KEY_BYTES: usize = 128;
/// Upper bound on rows in a record-list value.
pub const MAX_RUNTIME_FACT_RECORD_ROWS: usize = 256;
/// Upper bound on fields per record-list row.
pub const MAX_RUNTIME_FACT_RECORD_FIELDS: usize = 64;
/// Upper bound on the serialized snapshot part carried by one `runtime.fact_snapshot`
/// event: the same single-event payload bound the ledger applies to a
/// `fact.observed` event. A larger store is sealed as several parts, never truncated.
pub const MAX_RUNTIME_FACT_SNAPSHOT_BYTES: usize = crate::fact::MAX_FACT_OBSERVATION_BYTES;
/// Shortest period between two periodic `runtime.fact_snapshot` events.
pub const RUNTIME_FACT_SNAPSHOT_INTERVAL_MS: u64 = 60_000;
/// Key of the runtime fact that carries the manifest's subsystem rows.
pub const CONFIG_SUBSYSTEMS_FACT_KEY: &str = "config.subsystems";
/// Key prefix of the runtime facts that carry the manifest's parameter rows: one fact
/// `config.parameters.<section>` per section, the section being a key's first segment
/// (Workflow #387 D1b, model v1.2 §5.9; the one `config.parameters` fact would pass its row
/// bound).
pub const CONFIG_PARAMETERS_FACT_PREFIX: &str = "config.parameters.";
/// The section of a parameter key without a dot.
pub const CONFIG_TOP_LEVEL_SECTION: &str = "top_level";
/// Key of the runtime fact that names the configuration layers a start read (Workflow #387 D1b,
/// model v1.2 §5.7): one row with the configuration file's and the defaults file's SHA-256 and,
/// for an installed Runtime, the configuration generation.
pub const CONFIG_SELECTION_FACT_KEY: &str = "config.selection";
/// Key of the runtime fact that holds the settings an instruction changed (Workflow #387 D1b,
/// model v1.2 §5.7): one row per overridden key, restored at start.
pub const CONFIG_OVERRIDES_FACT_KEY: &str = "host.config_overrides";
/// Upper bound on subsystems in one configuration manifest.
pub const MAX_CONFIG_MANIFEST_SUBSYSTEMS: usize = 64;
/// Upper bound on the parameters of one section of a configuration manifest: the rows of one
/// `config.parameters.<section>` fact.
pub const MAX_CONFIG_MANIFEST_PARAMETERS: usize = 256;
/// Upper bound on a subsystem name or a parameter key, in bytes.
pub const MAX_CONFIG_MANIFEST_NAME_BYTES: usize = 128;
/// The `config.subsystems` row field that names, comma-separated, the `variable` of every
/// `note` link of the subsystem's outcome: the ignored environment overrides of `env_overrides`
/// (Workflow #318 cfg3; Workflow #378 A3, Fail Loud).
pub const CONFIG_SUBSYSTEM_IGNORED_VARIABLES_FIELD: &str = "ignored_variables";
/// The outcome value key a noted environment variable carries (`env_override_ignored`).
const NOTED_VARIABLE_KEY: &str = "variable";
/// Key of the instance-scoped runtime fact that carries the package name the device
/// reported in the foreground (slice #316-B3).
pub const APPLICATION_FOREGROUND_FACT_KEY: &str = "application.foreground";
/// Key families accepted for runtime facts. None of them overlaps the
/// instance-fact families validated by `crate::fact`.
pub const RUNTIME_FACT_FAMILIES: [&str; 9] = [
    "device.",
    "backend.",
    "task.",
    "host.",
    "lease.",
    "config.",
    "provider.",
    "eligible.",
    "application.",
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
        return Err(SanitizationError::new(
            SanitizationCode::InvalidRuntimeFactKey,
            "key",
        ));
    }
    if !RUNTIME_FACT_FAMILIES
        .iter()
        .any(|family| key.starts_with(family))
    {
        return Err(SanitizationError::new(
            SanitizationCode::RuntimeFactFamilyUnknown,
            "key",
        ));
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
                SanitizationCode::RuntimeFactRecordListTooLarge,
                "value",
            ));
        }
        if self.ttl_ms == Some(0) {
            return Err(SanitizationError::new(
                SanitizationCode::InvalidRuntimeFactTtl,
                "ttl_ms",
            ));
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

/// The section of a configuration parameter key: its first segment, or
/// [`CONFIG_TOP_LEVEL_SECTION`] for a key without a dot (Workflow #387 D1b).
pub fn config_parameter_section(key: &str) -> &str {
    key.split_once('.')
        .map_or(CONFIG_TOP_LEVEL_SECTION, |(section, _)| section)
}

/// The key of the runtime fact that carries one section's parameter rows.
pub fn config_parameters_fact_key(section: &str) -> String {
    format!("{CONFIG_PARAMETERS_FACT_PREFIX}{section}")
}

/// The in-memory runtime configuration manifest: which subsystems the host
/// runs and why, and the effective value of every parameter it applies, each
/// marked with where the value came from and whether an instruction may change
/// it while the Runtime runs. It is data only; the host records it as the
/// `config.subsystems` fact and one `config.parameters.<section>` fact per
/// section at startup, and records a section again when an instruction changes
/// it (Workflow #387 D1b).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeConfigManifest {
    pub subsystems: Vec<ConfigSubsystem>,
    pub parameters: Vec<ConfigParameter>,
}

/// One subsystem the host either runs or leaves out. `outcome` says why, as a code with
/// values (ledger revision 3, model section 8.1; was the prose `reason`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigSubsystem {
    pub name: String,
    pub enabled: bool,
    pub outcome: crate::outcome::Outcome,
}

/// One effective parameter value, where it came from, and whether an
/// instruction may change it while the Runtime runs (Workflow #387 D1b, `settable`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigParameter {
    pub key: String,
    pub value: FactScalar,
    pub source: ConfigParameterSource,
    pub settable: bool,
}

/// Where an effective parameter value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigParameterSource {
    /// Set in the configuration file.
    Explicit,
    /// A library default; nothing in the configuration file named it.
    Default,
    /// Learned at startup from the environment (for example discovery).
    Discovered,
    /// Set by a `ConfigureSettings` instruction while the Runtime runs (Workflow #387 D1b).
    Instruction,
}

impl ConfigParameterSource {
    /// The wire spelling, also used as the `source` row field.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Explicit => "explicit",
            Self::Default => "default",
            Self::Discovered => "discovered",
            Self::Instruction => "instruction",
        }
    }
}

/// The configuration layers one start read (Workflow #387 D1b, model v1.2 §5.7), recorded as
/// `config.selection`: the configuration file's and the defaults file's `sha256:<hex>` and, for
/// an installed Runtime, the configuration generation acsetup committed. Two starts with
/// different selections read different file layers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigSelection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_generation: Option<u64>,
    pub config_sha256: String,
    pub defaults_sha256: String,
}

impl ConfigSelection {
    /// The `config.selection` fact: one row `{config_sha256, defaults_sha256}` with
    /// `config_generation` (decimal text) when the Runtime is installed.
    pub fn to_fact_record(
        &self,
        observed_at_unix_ms: u64,
        source: OriginModule,
    ) -> RuntimeFactRecord {
        let mut row = BTreeMap::from([
            (
                "config_sha256".to_owned(),
                FactScalar::String(self.config_sha256.clone()),
            ),
            (
                "defaults_sha256".to_owned(),
                FactScalar::String(self.defaults_sha256.clone()),
            ),
        ]);
        if let Some(generation) = self.config_generation {
            row.insert(
                "config_generation".to_owned(),
                FactScalar::String(generation.to_string()),
            );
        }
        RuntimeFactRecord {
            scope: RuntimeFactScope::Runtime,
            key: CONFIG_SELECTION_FACT_KEY.to_owned(),
            value: FactValue::RecordList(vec![row]),
            observed_at_unix_ms,
            source,
            ttl_ms: None,
        }
    }
}

impl RuntimeConfigManifest {
    /// Checks the subsystem bound, the per-section parameter bound and every
    /// name and key: non-empty, control-free, within the byte bounds, and a
    /// parameter key whose section fact key is a valid runtime fact key. A
    /// manifest that passes here always yields records that pass
    /// [`RuntimeFactRecord::validate`].
    pub fn validate(&self) -> Result<(), SanitizationError> {
        if self.subsystems.len() > MAX_CONFIG_MANIFEST_SUBSYSTEMS {
            return Err(SanitizationError::new(
                SanitizationCode::ConfigManifestSubsystemsTooMany,
                "subsystems",
            ));
        }
        for subsystem in &self.subsystems {
            if !manifest_text_ok(&subsystem.name, MAX_CONFIG_MANIFEST_NAME_BYTES) {
                return Err(SanitizationError::new(
                    SanitizationCode::InvalidConfigSubsystemName,
                    "subsystems",
                ));
            }
        }
        let mut per_section = BTreeMap::<&str, usize>::new();
        for parameter in &self.parameters {
            let section = config_parameter_section(&parameter.key);
            if !manifest_text_ok(&parameter.key, MAX_CONFIG_MANIFEST_NAME_BYTES)
                || validate_runtime_fact_key(&config_parameters_fact_key(section)).is_err()
            {
                return Err(SanitizationError::new(
                    SanitizationCode::InvalidConfigParameterKey,
                    "parameters",
                ));
            }
            let count = per_section.entry(section).or_default();
            *count += 1;
            if *count > MAX_CONFIG_MANIFEST_PARAMETERS {
                return Err(SanitizationError::new(
                    SanitizationCode::ConfigManifestParametersTooMany,
                    "parameters",
                ));
            }
        }
        Ok(())
    }

    /// The sections of the parameters, each once, in name order.
    pub fn parameter_sections(&self) -> Vec<&str> {
        self.parameters
            .iter()
            .map(|parameter| config_parameter_section(&parameter.key))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    /// One `config.parameters.<section>` fact per section, in section name order, each with
    /// the rows of [`Self::parameter_section_fact_record`].
    pub fn parameter_fact_records(
        &self,
        observed_at_unix_ms: u64,
        source: OriginModule,
    ) -> Vec<RuntimeFactRecord> {
        self.parameter_sections()
            .into_iter()
            .map(|section| self.parameter_section_fact_record(section, observed_at_unix_ms, source))
            .collect()
    }

    /// The `config.parameters.<section>` fact of one section, in scope `runtime` with no
    /// lifetime: one row `{key, value, source, settable}` per parameter of the section, in
    /// manifest order; no row for a section the manifest does not have. Pure.
    pub fn parameter_section_fact_record(
        &self,
        section: &str,
        observed_at_unix_ms: u64,
        source: OriginModule,
    ) -> RuntimeFactRecord {
        let rows = self
            .parameters
            .iter()
            .filter(|parameter| config_parameter_section(&parameter.key) == section)
            .map(|parameter| {
                BTreeMap::from([
                    ("key".to_owned(), FactScalar::String(parameter.key.clone())),
                    ("value".to_owned(), parameter.value.clone()),
                    (
                        "source".to_owned(),
                        FactScalar::String(parameter.source.as_str().to_owned()),
                    ),
                    (
                        "settable".to_owned(),
                        FactScalar::Boolean(parameter.settable),
                    ),
                ])
            })
            .collect();
        RuntimeFactRecord {
            scope: RuntimeFactScope::Runtime,
            key: config_parameters_fact_key(section),
            value: FactValue::RecordList(rows),
            observed_at_unix_ms,
            source,
            ttl_ms: None,
        }
    }

    /// Encodes the manifest's subsystems as the `config.subsystems` fact, in
    /// scope `runtime` with no lifetime: one row `{name, enabled, code}` per
    /// subsystem, plus the scalar values of its outcome under their keys
    /// (ledger revision 3) and, when its outcome has `note` links with a
    /// `variable`, those names comma-separated under `ignored_variables`
    /// (never silent). Pure: nothing is sampled or appended here.
    pub fn subsystems_fact_record(
        &self,
        observed_at_unix_ms: u64,
        source: OriginModule,
    ) -> RuntimeFactRecord {
        let subsystems = self
            .subsystems
            .iter()
            .map(|subsystem| {
                let mut row = BTreeMap::from([
                    (
                        "name".to_owned(),
                        FactScalar::String(subsystem.name.clone()),
                    ),
                    ("enabled".to_owned(), FactScalar::Boolean(subsystem.enabled)),
                    (
                        "code".to_owned(),
                        FactScalar::String(subsystem.outcome.code().as_str().to_owned()),
                    ),
                ]);
                for (key, value) in subsystem.outcome.values() {
                    let scalar = match value {
                        serde_json::Value::Bool(flag) => FactScalar::Boolean(*flag),
                        serde_json::Value::Number(number) => match number.as_i64() {
                            Some(integer) => FactScalar::Integer(integer),
                            None => continue,
                        },
                        serde_json::Value::String(text) => FactScalar::String(text.clone()),
                        _ => continue,
                    };
                    row.entry(key.clone()).or_insert(scalar);
                }
                // Fail Loud: the `variable` of every `note` link (each ignored environment
                // override of `env_overrides`, `env_override_ignored`) is named in the row, in
                // link order and comma-separated, so the startup record never drops one.
                let noted = subsystem
                    .outcome
                    .causes()
                    .iter()
                    .filter(|link| {
                        link.relation().as_str() == crate::outcome::CauseRelation::Note.as_str()
                    })
                    .filter_map(|link| {
                        link.outcome()
                            .value(NOTED_VARIABLE_KEY)
                            .and_then(serde_json::Value::as_str)
                    })
                    .collect::<Vec<_>>();
                if !noted.is_empty() {
                    row.insert(
                        CONFIG_SUBSYSTEM_IGNORED_VARIABLES_FIELD.to_owned(),
                        FactScalar::String(noted.join(",")),
                    );
                }
                row
            })
            .collect();
        RuntimeFactRecord {
            scope: RuntimeFactScope::Runtime,
            key: CONFIG_SUBSYSTEMS_FACT_KEY.to_owned(),
            value: FactValue::RecordList(subsystems),
            observed_at_unix_ms,
            source,
            ttl_ms: None,
        }
    }
}

/// Non-empty, control-free, and at most `max_bytes` long.
fn manifest_text_ok(value: &str, max_bytes: usize) -> bool {
    !value.is_empty() && value.len() <= max_bytes && !value.chars().any(char::is_control)
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
    /// An ADB command against the instance failed: the ADB baseline is lost, so every
    /// device-bound observation is stale until ADB answers again.
    AdbUnreachable,
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

impl RuntimeFactInvalidation {
    /// Checks the dropped key; the reason and time are closed types.
    pub fn validate(&self) -> Result<(), SanitizationError> {
        validate_runtime_fact_key(&self.key)
    }
}

/// Sealed image of the store at one ledger position: the whole store, or one part of a
/// snapshot set (Workflow #308 5d-1). Every part of one set shares `snapshot_id`
/// (= `ledger_position`) and `parts`; `part` counts from 1. A snapshot sealed before parts
/// existed reads as part 1 of 1 with `snapshot_id` 0.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeFactSnapshot {
    pub schema_version: String,
    pub ledger_position: u64,
    pub taken_at_unix_ms: u64,
    #[serde(default)]
    pub snapshot_id: u64,
    #[serde(default = "first_snapshot_part")]
    pub part: u16,
    #[serde(default = "first_snapshot_part")]
    pub parts: u16,
    pub records: Vec<RuntimeFactRecord>,
}

const fn first_snapshot_part() -> u16 {
    1
}

impl RuntimeFactSnapshot {
    /// Checks the schema identity, the ledger position, `1 <= part <= parts`, the record
    /// bound, and every record. Read-safe: never re-serializes, so a part sealed by an older
    /// Runtime stays valid after an additive field (Workflow #191 B1-S1).
    pub fn validate(&self) -> Result<(), SanitizationError> {
        if self.schema_version != RUNTIME_FACT_SCHEMA_VERSION {
            return Err(SanitizationError::new(
                SanitizationCode::RuntimeFactSchemaVersionMismatch,
                "schema_version",
            ));
        }
        if self.ledger_position == 0 {
            return Err(SanitizationError::new(
                SharedCode::RuntimeFactLedgerPositionInvalid,
                "ledger_position",
            ));
        }
        if self.part == 0 || self.part > self.parts {
            return Err(SanitizationError::new(
                SanitizationCode::RuntimeFactSnapshotPartInvalid,
                "part",
            ));
        }
        if self.records.len() > MAX_RUNTIME_FACTS {
            return Err(SanitizationError::new(
                SanitizationCode::RuntimeFactSnapshotTooLarge,
                "records",
            ));
        }
        self.records
            .iter()
            .try_for_each(RuntimeFactRecord::validate)
    }

    /// [`Self::validate`], then the serialized size one ledger event may carry, measured on
    /// the encoding about to be stored. Write side only (sealing a part,
    /// `EventPayload::validate_write_bounds`); never call it on a decoded record.
    pub fn validate_for_append(&self) -> Result<(), SanitizationError> {
        self.validate()?;
        if serde_json::to_vec(self)
            .map_err(|_| SanitizationError::new(SharedCode::InvalidRuntimeFactSnapshot, "records"))?
            .len()
            > MAX_RUNTIME_FACT_SNAPSHOT_BYTES
        {
            return Err(SanitizationError::new(
                SharedCode::RuntimeFactSnapshotPayloadTooLarge,
                "records",
            ));
        }
        Ok(())
    }
}

/// An incomplete snapshot set passed over by [`newest_complete_runtime_fact_snapshot_set`]:
/// `parts_found` contiguous parts of `snapshot_id` out of its `parts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SkippedRuntimeFactSnapshotSet {
    pub snapshot_id: u64,
    pub parts_found: u16,
    pub parts: u16,
}

/// Outcome of [`newest_complete_runtime_fact_snapshot_set`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RuntimeFactSnapshotSetSearch {
    /// The newest complete set in part order, each part with its ledger sequence; `None`
    /// when the ledger holds no complete set.
    pub set: Option<Vec<(u64, RuntimeFactSnapshot)>>,
    /// Every incomplete set passed over before it, newest first.
    pub skipped: Vec<SkippedRuntimeFactSnapshotSet>,
}

/// Workflow #308 5d-1: finds the newest complete snapshot set among `runtime.fact_snapshot`
/// parts that `next_newest` yields newest first, each with its ledger sequence. A set is
/// complete when its newest part is part `parts` and the parts before it are `parts - 1`
/// down to 1 of the same `snapshot_id` and `parts`, contiguous among the snapshot events. A
/// set that is not (a stop while sealing) is skipped and the search continues with the next
/// older part. Nothing older than the complete set is read.
pub fn newest_complete_runtime_fact_snapshot_set<E>(
    mut next_newest: impl FnMut() -> Result<Option<(u64, RuntimeFactSnapshot)>, E>,
) -> Result<RuntimeFactSnapshotSetSearch, E> {
    let mut search = RuntimeFactSnapshotSetSearch::default();
    let mut pending = next_newest()?;
    while let Some(newest) = pending.take() {
        let (snapshot_id, part, parts) = (newest.1.snapshot_id, newest.1.part, newest.1.parts);
        let mut set = vec![newest];
        let mut expected = part;
        while expected > 1 {
            match next_newest()? {
                Some(older)
                    if (older.1.snapshot_id, older.1.part, older.1.parts)
                        == (snapshot_id, expected - 1, parts) =>
                {
                    set.push(older);
                    expected -= 1;
                }
                other => {
                    pending = other;
                    break;
                }
            }
        }
        if expected == 1 {
            if part == parts {
                set.reverse();
                search.set = Some(set);
                return Ok(search);
            }
            pending = next_newest()?;
        }
        search.skipped.push(SkippedRuntimeFactSnapshotSet {
            snapshot_id,
            parts_found: part - expected + 1,
            parts,
        });
    }
    Ok(search)
}
