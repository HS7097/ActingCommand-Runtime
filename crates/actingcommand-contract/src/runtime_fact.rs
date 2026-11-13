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
/// Key of the runtime fact that names the active policy catalog generation (Workflow #389
/// MCP-9a, register A5): one row, [`ConfigCatalog`].
pub const CONFIG_CATALOG_FACT_KEY: &str = "config.catalog";
/// Key of the runtime fact that holds the compile warnings of the active composed catalog
/// generation (#428 CW-1, register §1.290): [`ConfigCatalogWarnings`]. Absent while no composed
/// generation had a warning; recorded empty once the active one has none.
pub const CONFIG_CATALOG_WARNINGS_FACT_KEY: &str = "config.catalog_warnings";
/// Key of the runtime fact that holds who wrote the values of the active composed catalog
/// generation (#428 SCHED-C1b, register §1.299): [`ConfigCatalogProvenance`]. Absent while no
/// composed generation had a fixed value or a declared valuation; recorded empty once the active
/// one has none.
pub const CONFIG_CATALOG_PROVENANCE_FACT_KEY: &str = "config.catalog_provenance";
/// Key of the runtime fact that says what a start found (Workflow #454, register §1.342): one row
/// `{owner_epoch, first_start, tagged_state}`, the start's owner epoch, whether it found no
/// earlier start record (`runtime.started` or `runtime.takeover`, read before its own: the root's
/// first start, which writes no seeding marker) and whether it seeded or loaded the tagged state
/// (`seed`, `load`). Every start of a build that records it (0.12.59 and later) and has a
/// configuration manifest records it with the configuration facts, re-observed as every `config.`
/// fact is, so before that Runtime is ready the row names its own epoch. A start that does not
/// record it (a build before 0.12.59, after a rollback to one, or a start without a configuration
/// manifest) leaves the last row in place, which then names another epoch: a reader compares the
/// row's `owner_epoch` with the epoch it judges (`actingctl install` compares it with its S0's).
pub const CONFIG_START_FACT_KEY: &str = "config.start";
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

/// The prefix of the loaded-package table's workbench keys (Workflow #389 §6.5, register U3).
/// They are not tagged: `host.pack_registry.` is re-observed every session.
pub const PACK_REGISTRY_FACT_PREFIX: &str = "host.pack_registry.";
/// The key of an instance's startup package entry (instance scope, register U3).
pub const PACK_REGISTRY_STARTUP_FACT_KEY: &str = "host.pack_registry.startup";
/// The prefix of a prerequisite package's entry (runtime scope, register U3).
pub const PACK_REGISTRY_PREREQUISITE_FACT_PREFIX: &str = "host.pack_registry.prerequisite.";

/// Which entry of the loaded-package table a workbench record names (Workflow #389 §6.5,
/// register U3): a procedure binding by its `package_id` (runtime scope,
/// `host.pack_registry.<package_id>`), an instance's startup package (instance scope,
/// `host.pack_registry.startup`) or a prerequisite package by its `package_id` (runtime scope,
/// `host.pack_registry.prerequisite.<package_id>`). U-3 writes the records; a value never
/// carries a package digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackRegistryFactKey {
    Procedure { package_id: String },
    Startup { instance_id: InstanceId },
    Prerequisite { package_id: String },
}

impl PackRegistryFactKey {
    /// Where the entry's record applies.
    pub fn scope(&self) -> RuntimeFactScope {
        match self {
            Self::Procedure { .. } | Self::Prerequisite { .. } => RuntimeFactScope::Runtime,
            Self::Startup { instance_id } => RuntimeFactScope::Instance {
                instance_id: *instance_id,
            },
        }
    }

    /// The entry's workbench key. A procedure's `package_id` that would read back as a
    /// prerequisite key is refused, as is a key no runtime fact may have.
    pub fn fact_key(&self) -> Result<String, SanitizationError> {
        let key = match self {
            Self::Procedure { package_id } => {
                if package_id.starts_with("prerequisite.") {
                    return Err(SanitizationError::new(
                        SanitizationCode::InvalidRuntimeFactKey,
                        "key",
                    ));
                }
                format!("{PACK_REGISTRY_FACT_PREFIX}{package_id}")
            }
            Self::Startup { .. } => PACK_REGISTRY_STARTUP_FACT_KEY.to_owned(),
            Self::Prerequisite { package_id } => {
                format!("{PACK_REGISTRY_PREREQUISITE_FACT_PREFIX}{package_id}")
            }
        };
        if Self::package_id_text(&key).is_empty() {
            return Err(SanitizationError::new(
                SanitizationCode::InvalidRuntimeFactKey,
                "key",
            ));
        }
        validate_runtime_fact_key(&key)?;
        Ok(key)
    }

    /// The entry a workbench record of `scope` and `key` names: the reader of the key the
    /// constructors build.
    pub fn from_fact(scope: &RuntimeFactScope, key: &str) -> Result<Self, SanitizationError> {
        validate_runtime_fact_key(key)?;
        let invalid = || SanitizationError::new(SanitizationCode::InvalidRuntimeFactKey, "key");
        let rest = key
            .strip_prefix(PACK_REGISTRY_FACT_PREFIX)
            .ok_or_else(invalid)?;
        let entry = match scope {
            RuntimeFactScope::Instance { instance_id } if key == PACK_REGISTRY_STARTUP_FACT_KEY => {
                Self::Startup {
                    instance_id: *instance_id,
                }
            }
            RuntimeFactScope::Instance { .. } => return Err(invalid()),
            RuntimeFactScope::Runtime => match rest.strip_prefix("prerequisite.") {
                Some(package_id) if !package_id.is_empty() => Self::Prerequisite {
                    package_id: package_id.to_owned(),
                },
                Some(_) => return Err(invalid()),
                None if !rest.is_empty() => Self::Procedure {
                    package_id: rest.to_owned(),
                },
                None => return Err(invalid()),
            },
        };
        Ok(entry)
    }

    fn package_id_text(key: &str) -> &str {
        key.strip_prefix(PACK_REGISTRY_PREREQUISITE_FACT_PREFIX)
            .or_else(|| key.strip_prefix(PACK_REGISTRY_FACT_PREFIX))
            .unwrap_or_default()
    }
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

/// The active policy catalog generation (Workflow #389 MCP-9a, register A5), recorded as
/// `config.catalog` (runtime scope, re-observed like every `config.` fact): its id, version and
/// hash, the `catalog.activated` or `catalog.rolled_back` record that made it active, the latest
/// approval decision about it and the generation it replaced. In the row the versions are
/// decimal text, as `config.selection`'s generation is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigCatalog {
    pub catalog_id: String,
    pub catalog_version: u64,
    pub catalog_hash: String,
    /// The event id of the record that made the generation active.
    pub activated_event_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_disposition: Option<crate::ApprovalDisposition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_catalog_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_catalog_version: Option<u64>,
}

impl ConfigCatalog {
    /// The `config.catalog` fact: one row of text fields; the optional ones only when present.
    pub fn to_fact_record(
        &self,
        observed_at_unix_ms: u64,
        source: OriginModule,
    ) -> RuntimeFactRecord {
        let text = |value: &str| FactScalar::String(value.to_owned());
        let mut row = BTreeMap::from([
            ("catalog_id".to_owned(), text(&self.catalog_id)),
            (
                "catalog_version".to_owned(),
                text(&self.catalog_version.to_string()),
            ),
            ("catalog_hash".to_owned(), text(&self.catalog_hash)),
            (
                "activated_event_id".to_owned(),
                text(&self.activated_event_id),
            ),
        ]);
        let optional = [
            ("approval_id", self.approval_id.clone()),
            (
                "approval_disposition",
                self.approval_disposition
                    .map(|disposition| disposition.as_str().to_owned()),
            ),
            ("previous_catalog_id", self.previous_catalog_id.clone()),
            (
                "previous_catalog_version",
                self.previous_catalog_version
                    .map(|version| version.to_string()),
            ),
        ];
        for (field, value) in optional {
            if let Some(value) = value {
                row.insert(field.to_owned(), FactScalar::String(value));
            }
        }
        RuntimeFactRecord {
            scope: RuntimeFactScope::Runtime,
            key: CONFIG_CATALOG_FACT_KEY.to_owned(),
            value: FactValue::RecordList(vec![row]),
            observed_at_unix_ms,
            source,
            ttl_ms: None,
        }
    }

    /// The generation a `config.catalog` record names; a record that is not the one row
    /// [`Self::to_fact_record`] writes is `invalid_fact_snapshot_record`.
    pub fn from_fact_record(record: &RuntimeFactRecord) -> Result<Self, SanitizationError> {
        let refused = || {
            SanitizationError::new(
                SanitizationCode::InvalidFactSnapshotRecord,
                "config_catalog",
            )
        };
        let FactValue::RecordList(rows) = &record.value else {
            return Err(refused());
        };
        let [row] = rows.as_slice() else {
            return Err(refused());
        };
        let text = |field: &str| -> Result<Option<String>, SanitizationError> {
            match row.get(field) {
                None => Ok(None),
                Some(FactScalar::String(value)) => Ok(Some(value.clone())),
                Some(_) => Err(refused()),
            }
        };
        let version = |field: &str| -> Result<Option<u64>, SanitizationError> {
            text(field)?
                .map(|value| value.parse::<u64>().map_err(|_| refused()))
                .transpose()
        };
        let disposition = text("approval_disposition")?
            .map(|value| {
                crate::ApprovalDisposition::ALL
                    .into_iter()
                    .find(|disposition| disposition.as_str() == value)
                    .ok_or_else(refused)
            })
            .transpose()?;
        Ok(Self {
            catalog_id: text("catalog_id")?.ok_or_else(refused)?,
            catalog_version: version("catalog_version")?.ok_or_else(refused)?,
            catalog_hash: text("catalog_hash")?.ok_or_else(refused)?,
            activated_event_id: text("activated_event_id")?.ok_or_else(refused)?,
            approval_id: text("approval_id")?,
            approval_disposition: disposition,
            previous_catalog_id: text("previous_catalog_id")?,
            previous_catalog_version: version("previous_catalog_version")?,
        })
    }
}

/// One compile warning of the active composed catalog generation (#428 CW-1, register §1.290):
/// the spelling of its code, the game in whose `schedule.json` the composer placed it and its
/// JSON pointer in that file. A warning the composer cannot place in a game's file has no game
/// and keeps its pointer in the composed document.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct CatalogCompileWarning {
    pub warning: String,
    pub game: Option<String>,
    pub pointer: String,
}

/// How many compile warnings of one code one game's file gave (#428 CW-1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogWarningCount {
    pub warning: String,
    pub game: Option<String>,
    pub count: u64,
}

/// The member of the cut row of `config.catalog_warnings` that says how many count rows the
/// fact's row bound left out (#428 CW-1 fix 1).
const CATALOG_WARNINGS_COUNTS_OMITTED: &str = "counts_omitted";
/// The member of the cut row of `config.catalog_warnings` that says how many place rows the
/// fact's row bound left out (#428 CW-1 fix 1).
const CATALOG_WARNINGS_PLACES_OMITTED: &str = "places_omitted";

/// The compile warnings of the active composed catalog generation (#428 CW-1, register §1.290),
/// recorded as `config.catalog_warnings` (runtime scope, re-observed like every `config.` fact):
/// first one count row `{warning, game?, count}` per code and game, then one place row
/// `{warning, game?, pointer}` per warning. Fix 1 (Fail Loud): rows past the fact's row bound are
/// never left out silently: the place rows are cut first, then the count rows, and one last cut
/// row `{counts_omitted, places_omitted}` says how many of each are missing. Empty while the
/// generation has no warning.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfigCatalogWarnings {
    pub counts: Vec<CatalogWarningCount>,
    pub places: Vec<CatalogCompileWarning>,
    /// How many count rows the row bound left out (fix 1): their warnings are in no count.
    pub counts_omitted: u64,
    /// How many place rows the row bound left out (fix 1).
    pub places_omitted: u64,
}

impl ConfigCatalogWarnings {
    /// The warnings of `warnings`, each once: counted by code and game, and placed in the order
    /// code, game, pointer. When the rows pass [`MAX_RUNTIME_FACT_RECORD_ROWS`], one row is kept
    /// for the cut row and the rest hold the count rows first, then as many place rows as fit;
    /// [`Self::counts_omitted`] and [`Self::places_omitted`] say how many of each were left out.
    pub fn of(warnings: &[CatalogCompileWarning]) -> Self {
        let mut places = warnings.to_vec();
        places.sort();
        places.dedup();
        let mut counts = BTreeMap::<(&str, Option<&str>), u64>::new();
        for place in &places {
            *counts
                .entry((place.warning.as_str(), place.game.as_deref()))
                .or_default() += 1;
        }
        let mut counts = counts
            .into_iter()
            .map(|((warning, game), count)| CatalogWarningCount {
                warning: warning.to_owned(),
                game: game.map(str::to_owned),
                count,
            })
            .collect::<Vec<_>>();
        let (mut counts_omitted, mut places_omitted) = (0, 0);
        if counts.len() + places.len() > MAX_RUNTIME_FACT_RECORD_ROWS {
            let omitted = |rows: usize| u64::try_from(rows).unwrap_or(u64::MAX);
            // The cut row takes one row of the bound.
            let room = MAX_RUNTIME_FACT_RECORD_ROWS - 1;
            let counts_kept = counts.len().min(room);
            let places_kept = room - counts_kept;
            counts_omitted = omitted(counts.len().saturating_sub(counts_kept));
            places_omitted = omitted(places.len().saturating_sub(places_kept));
            counts.truncate(counts_kept);
            places.truncate(places_kept);
        }
        Self {
            counts,
            places,
            counts_omitted,
            places_omitted,
        }
    }

    /// Whether the generation has no warning.
    pub fn is_empty(&self) -> bool {
        self.counts.is_empty() && self.counts_omitted == 0
    }

    /// Whether the row bound left rows out (fix 1): the fact then ends with its cut row.
    pub fn is_cut(&self) -> bool {
        self.counts_omitted > 0 || self.places_omitted > 0
    }

    /// How many warnings the count rows count, placed or not; a warning whose count row was left
    /// out ([`Self::counts_omitted`]) is not in it.
    pub fn total(&self) -> u64 {
        self.counts.iter().map(|count| count.count).sum()
    }

    /// The `config.catalog_warnings` fact: the count rows, then the place rows, then, when rows
    /// were left out, the cut row `{counts_omitted, places_omitted}`.
    pub fn to_fact_record(
        &self,
        observed_at_unix_ms: u64,
        source: OriginModule,
    ) -> RuntimeFactRecord {
        let row = |warning: &str, game: Option<&str>| {
            let mut row =
                BTreeMap::from([("warning".to_owned(), FactScalar::String(warning.to_owned()))]);
            if let Some(game) = game {
                row.insert("game".to_owned(), FactScalar::String(game.to_owned()));
            }
            row
        };
        let counts = self.counts.iter().map(|count| {
            let mut counted = row(&count.warning, count.game.as_deref());
            counted.insert(
                "count".to_owned(),
                FactScalar::Integer(i64::try_from(count.count).unwrap_or(i64::MAX)),
            );
            counted
        });
        let places = self.places.iter().map(|place| {
            let mut placed = row(&place.warning, place.game.as_deref());
            placed.insert(
                "pointer".to_owned(),
                FactScalar::String(place.pointer.clone()),
            );
            placed
        });
        let omitted = |rows: u64| FactScalar::Integer(i64::try_from(rows).unwrap_or(i64::MAX));
        let cut = self.is_cut().then(|| {
            BTreeMap::from([
                (
                    CATALOG_WARNINGS_COUNTS_OMITTED.to_owned(),
                    omitted(self.counts_omitted),
                ),
                (
                    CATALOG_WARNINGS_PLACES_OMITTED.to_owned(),
                    omitted(self.places_omitted),
                ),
            ])
        });
        RuntimeFactRecord {
            scope: RuntimeFactScope::Runtime,
            key: CONFIG_CATALOG_WARNINGS_FACT_KEY.to_owned(),
            value: FactValue::RecordList(counts.chain(places).chain(cut).collect()),
            observed_at_unix_ms,
            source,
            ttl_ms: None,
        }
    }

    /// The warnings a `config.catalog_warnings` record holds; a row that is neither a count row,
    /// a place row nor, last, a cut row as [`Self::to_fact_record`] writes them is
    /// `invalid_fact_snapshot_record`.
    pub fn from_fact_record(record: &RuntimeFactRecord) -> Result<Self, SanitizationError> {
        let refused = || {
            SanitizationError::new(
                SanitizationCode::InvalidFactSnapshotRecord,
                "config_catalog_warnings",
            )
        };
        let FactValue::RecordList(rows) = &record.value else {
            return Err(refused());
        };
        let mut read = Self::default();
        for (index, row) in rows.iter().enumerate() {
            // Fix 1: the cut row, the last one, with its two members and nothing else.
            if row.contains_key(CATALOG_WARNINGS_COUNTS_OMITTED)
                || row.contains_key(CATALOG_WARNINGS_PLACES_OMITTED)
            {
                let omitted = |field: &str| match row.get(field) {
                    Some(FactScalar::Integer(value)) => {
                        u64::try_from(*value).map_err(|_| refused())
                    }
                    _ => Err(refused()),
                };
                read.counts_omitted = omitted(CATALOG_WARNINGS_COUNTS_OMITTED)?;
                read.places_omitted = omitted(CATALOG_WARNINGS_PLACES_OMITTED)?;
                if index + 1 != rows.len() || row.len() != 2 || !read.is_cut() {
                    return Err(refused());
                }
                continue;
            }
            let text = |field: &str| -> Result<Option<String>, SanitizationError> {
                match row.get(field) {
                    None => Ok(None),
                    Some(FactScalar::String(value)) => Ok(Some(value.clone())),
                    Some(_) => Err(refused()),
                }
            };
            let warning = text("warning")?.ok_or_else(refused)?;
            let game = text("game")?;
            match (row.get("count"), text("pointer")?) {
                (Some(FactScalar::Integer(count)), None) => {
                    let count = u64::try_from(*count)
                        .ok()
                        .filter(|count| *count > 0)
                        .ok_or_else(refused)?;
                    read.counts.push(CatalogWarningCount {
                        warning,
                        game,
                        count,
                    });
                }
                (None, Some(pointer)) => read.places.push(CatalogCompileWarning {
                    warning,
                    game,
                    pointer,
                }),
                _ => return Err(refused()),
            }
        }
        Ok(read)
    }
}

/// Which value of a composed catalog a `config.catalog_provenance` row names (#428 SCHED-C1b,
/// register §1.299): a judged rule's fixed value or a pool's declared valuation block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CatalogValueKind {
    FixedValue,
    Valuation,
}

impl CatalogValueKind {
    /// Every kind, in the order the fact lists them.
    pub const ALL: [Self; 2] = [Self::FixedValue, Self::Valuation];

    /// The spelling the fact and the overview carry.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FixedValue => "fixed_value",
            Self::Valuation => "valuation",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == text)
    }
}

/// Who wrote a value of a composed catalog (#428 SCHED-C1b): the pack's own hand (no origin
/// member in `schedule.json`) or the model's export (the origin member `"model"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CatalogValueOrigin {
    Pack,
    Model,
}

impl CatalogValueOrigin {
    /// Every origin, in the order the fact lists them.
    pub const ALL: [Self; 2] = [Self::Pack, Self::Model];

    /// The spelling the fact and the overview carry.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pack => "pack",
            Self::Model => "model",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|origin| origin.as_str() == text)
    }
}

/// One value of the active composed catalog generation by who wrote it (#428 SCHED-C1b): the
/// game whose `schedule.json` holds it, its kind, its origin and the file id of its rule or pool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogProvenanceEntry {
    pub game: String,
    pub kind: CatalogValueKind,
    pub origin: CatalogValueOrigin,
    pub id: String,
}

/// How many values of one kind one game's file holds from one origin (#428 SCHED-C1b).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogProvenanceCount {
    pub game: String,
    pub kind: CatalogValueKind,
    pub origin: CatalogValueOrigin,
    pub count: u64,
}

/// The member of the cut row of `config.catalog_provenance` that says how many count rows the
/// fact's row bound left out (#428 SCHED-C1b, as CW-1 fix 1).
const CATALOG_PROVENANCE_COUNTS_OMITTED: &str = "counts_omitted";
/// The member of the cut row of `config.catalog_provenance` that says how many id rows the fact's
/// row bound left out (#428 SCHED-C1b).
const CATALOG_PROVENANCE_IDS_OMITTED: &str = "ids_omitted";

/// Who wrote the values of the active composed catalog generation (#428 SCHED-C1b, register
/// §1.299; `contracts/standard-pack-schedule.md`, "Provenance"), recorded as
/// `config.catalog_provenance` (runtime scope, re-observed like every `config.` fact): first one
/// count row `{game, kind, origin, count}` per game, kind and origin that has a value, then one id
/// row `{game, kind, origin, id}` per rule or pool. As `config.catalog_warnings` (CW-1 fix 1), rows
/// past the fact's row bound are never left out silently: the id rows are cut first, then the
/// count rows, and one last cut row `{counts_omitted, ids_omitted}` says how many of each are
/// missing. Empty while the generation has no fixed value and no declared valuation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfigCatalogProvenance {
    pub counts: Vec<CatalogProvenanceCount>,
    pub ids: Vec<CatalogProvenanceEntry>,
    /// How many count rows the row bound left out: their values are in no count.
    pub counts_omitted: u64,
    /// How many id rows the row bound left out.
    pub ids_omitted: u64,
}

impl ConfigCatalogProvenance {
    /// The values of `entries`: counted by game, kind and origin, and listed in the order game,
    /// kind, origin, then as given (the composer gives each list in file order). When the rows
    /// pass [`MAX_RUNTIME_FACT_RECORD_ROWS`], one row is kept for the cut row and the rest hold the
    /// count rows first, then as many id rows as fit; [`Self::counts_omitted`] and
    /// [`Self::ids_omitted`] say how many of each were left out.
    pub fn of(entries: &[CatalogProvenanceEntry]) -> Self {
        let mut ids = entries.to_vec();
        // Stable: each list keeps the order it was given in.
        ids.sort_by(|left, right| {
            left.game
                .cmp(&right.game)
                .then(left.kind.cmp(&right.kind))
                .then(left.origin.cmp(&right.origin))
        });
        let mut counts = BTreeMap::<(&str, CatalogValueKind, CatalogValueOrigin), u64>::new();
        for entry in &ids {
            *counts
                .entry((entry.game.as_str(), entry.kind, entry.origin))
                .or_default() += 1;
        }
        let mut counts = counts
            .into_iter()
            .map(|((game, kind, origin), count)| CatalogProvenanceCount {
                game: game.to_owned(),
                kind,
                origin,
                count,
            })
            .collect::<Vec<_>>();
        let (mut counts_omitted, mut ids_omitted) = (0, 0);
        if counts.len() + ids.len() > MAX_RUNTIME_FACT_RECORD_ROWS {
            let omitted = |rows: usize| u64::try_from(rows).unwrap_or(u64::MAX);
            // The cut row takes one row of the bound.
            let room = MAX_RUNTIME_FACT_RECORD_ROWS - 1;
            let counts_kept = counts.len().min(room);
            let ids_kept = room - counts_kept;
            counts_omitted = omitted(counts.len().saturating_sub(counts_kept));
            ids_omitted = omitted(ids.len().saturating_sub(ids_kept));
            counts.truncate(counts_kept);
            ids.truncate(ids_kept);
        }
        Self {
            counts,
            ids,
            counts_omitted,
            ids_omitted,
        }
    }

    /// Whether the generation has no fixed value and no declared valuation.
    pub fn is_empty(&self) -> bool {
        self.counts.is_empty() && self.counts_omitted == 0
    }

    /// Whether the row bound left rows out: the fact then ends with its cut row.
    pub fn is_cut(&self) -> bool {
        self.counts_omitted > 0 || self.ids_omitted > 0
    }

    /// How many values of `kind` from `origin` the count rows count, over every game; a value
    /// whose count row was left out ([`Self::counts_omitted`]) is not in it.
    pub fn total(&self, kind: CatalogValueKind, origin: CatalogValueOrigin) -> u64 {
        self.counts
            .iter()
            .filter(|count| count.kind == kind && count.origin == origin)
            .map(|count| count.count)
            .sum()
    }

    /// The `config.catalog_provenance` fact: the count rows, then the id rows, then, when rows
    /// were left out, the cut row `{counts_omitted, ids_omitted}`.
    pub fn to_fact_record(
        &self,
        observed_at_unix_ms: u64,
        source: OriginModule,
    ) -> RuntimeFactRecord {
        let row = |game: &str, kind: CatalogValueKind, origin: CatalogValueOrigin| {
            BTreeMap::from([
                ("game".to_owned(), FactScalar::String(game.to_owned())),
                (
                    "kind".to_owned(),
                    FactScalar::String(kind.as_str().to_owned()),
                ),
                (
                    "origin".to_owned(),
                    FactScalar::String(origin.as_str().to_owned()),
                ),
            ])
        };
        let integer = |value: u64| FactScalar::Integer(i64::try_from(value).unwrap_or(i64::MAX));
        let counts = self.counts.iter().map(|count| {
            let mut counted = row(&count.game, count.kind, count.origin);
            counted.insert("count".to_owned(), integer(count.count));
            counted
        });
        let ids = self.ids.iter().map(|entry| {
            let mut listed = row(&entry.game, entry.kind, entry.origin);
            listed.insert("id".to_owned(), FactScalar::String(entry.id.clone()));
            listed
        });
        let cut = self.is_cut().then(|| {
            BTreeMap::from([
                (
                    CATALOG_PROVENANCE_COUNTS_OMITTED.to_owned(),
                    integer(self.counts_omitted),
                ),
                (
                    CATALOG_PROVENANCE_IDS_OMITTED.to_owned(),
                    integer(self.ids_omitted),
                ),
            ])
        });
        RuntimeFactRecord {
            scope: RuntimeFactScope::Runtime,
            key: CONFIG_CATALOG_PROVENANCE_FACT_KEY.to_owned(),
            value: FactValue::RecordList(counts.chain(ids).chain(cut).collect()),
            observed_at_unix_ms,
            source,
            ttl_ms: None,
        }
    }

    /// The values a `config.catalog_provenance` record holds; a row that is neither a count row,
    /// an id row nor, last, a cut row as [`Self::to_fact_record`] writes them is
    /// `invalid_fact_snapshot_record`.
    pub fn from_fact_record(record: &RuntimeFactRecord) -> Result<Self, SanitizationError> {
        let refused = || {
            SanitizationError::new(
                SanitizationCode::InvalidFactSnapshotRecord,
                "config_catalog_provenance",
            )
        };
        let FactValue::RecordList(rows) = &record.value else {
            return Err(refused());
        };
        let mut read = Self::default();
        for (index, row) in rows.iter().enumerate() {
            // The cut row, the last one, with its two members and nothing else.
            if row.contains_key(CATALOG_PROVENANCE_COUNTS_OMITTED)
                || row.contains_key(CATALOG_PROVENANCE_IDS_OMITTED)
            {
                let omitted = |field: &str| match row.get(field) {
                    Some(FactScalar::Integer(value)) => {
                        u64::try_from(*value).map_err(|_| refused())
                    }
                    _ => Err(refused()),
                };
                read.counts_omitted = omitted(CATALOG_PROVENANCE_COUNTS_OMITTED)?;
                read.ids_omitted = omitted(CATALOG_PROVENANCE_IDS_OMITTED)?;
                if index + 1 != rows.len() || row.len() != 2 || !read.is_cut() {
                    return Err(refused());
                }
                continue;
            }
            let text = |field: &str| match row.get(field) {
                Some(FactScalar::String(value)) => Ok(value.clone()),
                _ => Err(refused()),
            };
            let game = text("game")?;
            let kind = text("kind")?;
            let kind = CatalogValueKind::parse(&kind).ok_or_else(refused)?;
            let origin = text("origin")?;
            let origin = CatalogValueOrigin::parse(&origin).ok_or_else(refused)?;
            // A count row or an id row: the three members and one of `count` and `id`.
            if row.len() != 4 {
                return Err(refused());
            }
            match (row.get("count"), row.get("id")) {
                (Some(FactScalar::Integer(count)), None) => {
                    let count = u64::try_from(*count)
                        .ok()
                        .filter(|count| *count > 0)
                        .ok_or_else(refused)?;
                    read.counts.push(CatalogProvenanceCount {
                        game,
                        kind,
                        origin,
                        count,
                    });
                }
                (None, Some(FactScalar::String(id))) => read.ids.push(CatalogProvenanceEntry {
                    game,
                    kind,
                    origin,
                    id: id.clone(),
                }),
                _ => return Err(refused()),
            }
        }
        Ok(read)
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
