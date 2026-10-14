// SPDX-License-Identifier: AGPL-3.0-only

//! The `ledger_observer` section (Workflow #383 AO-1; `contracts/ledger-observer.md` §15,
//! `contracts/v012-register.md` §5). Every key is parsed, checked and defaulted from v0.12.0 on,
//! the keys only 0.12.x slices read included (order-v012 R4): the write path's retries, stall
//! limit, probe period and dump limit take effect with AO-1; the capture queue's budget, tiers,
//! batches and request ring with AO-2; the tag lists with AO-2 and AO-5; the probes' periods,
//! sample size, lag threshold and gap window with AO-8.
//!
//! The section is read whole and checked whole (M387 §5.3): the tiers rise strictly within
//! `(0, 1]`, the hysteresis lies within `(0, 1)` so each release line is below its activation
//! line, the gap between tiers 2 and 3 holds at least one spill batch, and the waits list one
//! wait per write attempt. A broken rule is `config_value_invalid` (`section_check`) for the key
//! it concerns, at start, in `check-config` and through an instruction alike. Every default lives
//! in the defaults file.

use crate::codes::ConfigValueProblem;
use crate::effective::ConfigFailure;
use crate::table::{
    DefaultSource, FilePath, KeyName, KeyRange, KeyRow, NotSettable, Settable, ValueType,
};
use actingcommand_contract::FactScalar;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The section's name: the first segment of its keys.
pub const SECTION: &str = "ledger_observer";
/// The capture queue's memory segment byte limit (§7; AO-2).
pub const BUFFER_BYTES: &str = "ledger_observer.buffer_bytes";
/// The capture queue's memory segment record limit (§7; AO-2).
pub const BUFFER_RECORDS: &str = "ledger_observer.buffer_records";
/// Tier 1 of the capture queue (§7; AO-2).
pub const BUFFER_TIER1_RATIO: &str = "ledger_observer.buffer.tier1_ratio";
/// Tier 2: spilling starts (§7; AO-2).
pub const BUFFER_TIER2_RATIO: &str = "ledger_observer.buffer.tier2_ratio";
/// Tier 3: alarm, everything spills (§7; AO-2).
pub const BUFFER_TIER3_RATIO: &str = "ledger_observer.buffer.tier3_ratio";
/// Release line = tier × (1 − hysteresis) (§7; AO-2).
pub const BUFFER_HYSTERESIS_RATIO: &str = "ledger_observer.buffer.hysteresis_ratio";
/// Bytes per spill (§7; AO-2).
pub const SPILL_BATCH_BYTES: &str = "ledger_observer.spill_batch_bytes";
/// The dump file's limit (§9.2; AO-1).
pub const DUMP_MAX_BYTES: &str = "ledger_observer.dump_max_bytes";
/// Records per flush batch below tier 1 (§8; AO-2).
pub const FLUSH_BATCH_RECORDS: &str = "ledger_observer.flush_batch_records";
/// Records per flush batch at tier 1 and above (§8; AO-2).
pub const FLUSH_BATCH_RECORDS_BUSY: &str = "ledger_observer.flush_batch_records_busy";
/// Attempts of one write, the first included (§9.1; AO-1).
pub const WRITE_ATTEMPTS: &str = "ledger_observer.write_attempts";
/// The wait before each attempt, in milliseconds (§9.1; AO-1).
pub const WRITE_RETRY_BACKOFF_MS: &str = "ledger_observer.write_retry_backoff_ms";
/// One attempt's stall limit (§9.1, T8; AO-1).
pub const FLUSH_ATTEMPT_STALL_MS: &str = "ledger_observer.flush_attempt_stall_ms";
/// The flush lag threshold of probe T4 (§14; AO-8).
pub const FLUSH_LAG_WARN_MS: &str = "ledger_observer.flush_lag_warn_ms";
/// The period of probes T3, T4, T5 and T8 (§14; T8 with AO-1).
pub const HEALTH_PROBE_INTERVAL_MS: &str = "ledger_observer.health_probe_interval_ms";
/// The period of probes T1, T2 and T7 (§14; AO-8).
pub const PROBE_INTERVAL_MS: &str = "ledger_observer.probe_interval_ms";
/// Keys per T1 sample (§14; AO-8).
pub const PROBE_SAMPLE_KEYS: &str = "ledger_observer.probe_sample_keys";
/// The ring size of the index table's request rows (§11.1; AO-2, AO-6).
pub const REQUEST_INDEX_RECORDS: &str = "ledger_observer.request_index_records";
/// The window of "recent gaps" in status (§10; AO-8).
pub const STATUS_GAP_WINDOW_MS: &str = "ledger_observer.status_gap_window_ms";
/// The tagged workbench keys (§11.2; AO-2, AO-5).
pub const TAG_WORKBENCH_PATTERNS: &str = "ledger_observer.tag.workbench_patterns";
/// The workbench keys known to be untagged and re-observed (§11.2; AO-5).
pub const TAG_REOBSERVE_PREFIXES: &str = "ledger_observer.tag.reobserve_prefixes";

/// The largest attempt count a configuration may name: the write path counts in `u32`.
const MAX_ATTEMPTS: i64 = u32::MAX as i64;

/// The section as the configuration file and the defaults file write it: every member may be
/// left out and none is unknown (M387 §5.3). Each value keeps its JSON here; the key table
/// checks its type and range, the whole-section check its rules.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerObserverFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    buffer_bytes: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    buffer_records: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    buffer: Option<BufferFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    spill_batch_bytes: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    dump_max_bytes: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    flush_batch_records: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    flush_batch_records_busy: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    write_attempts: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    write_retry_backoff_ms: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    flush_attempt_stall_ms: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    flush_lag_warn_ms: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    health_probe_interval_ms: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    probe_interval_ms: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    probe_sample_keys: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    request_index_records: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    status_gap_window_ms: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tag: Option<TagFile>,
}

/// The `buffer` member: the capture queue's tiers.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BufferFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tier1_ratio: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tier2_ratio: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tier3_ratio: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    hysteresis_ratio: Option<Value>,
}

/// The `tag` member: which workbench keys are tagged.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TagFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    workbench_patterns: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reobserve_prefixes: Option<Value>,
}

/// Read when the ledger opens: the capture queue, the writer and its probe exist before any
/// instruction value is restored (§15: "the queue exists before the ledger opens").
const AT_OPEN: Settable = Settable::No(NotSettable::BeforeLedgerOpen);
/// Read where it is used, by a probe or a status read (§15: "probe periods true").
const WHEN_USED: Settable = Settable::Yes;
/// At least one, in the key's unit.
const POSITIVE: KeyRange = KeyRange::Integer {
    min: 1,
    max: i64::MAX,
};

const fn row(
    name: &'static str,
    value_type: ValueType,
    range: KeyRange,
    settable: Settable,
    reader: &'static str,
) -> KeyRow {
    KeyRow {
        name: KeyName::Exact(name),
        file_path: FilePath::Key,
        value_type,
        range,
        default: DefaultSource::File,
        settable,
        reader,
        since: "0.12.0",
    }
}

/// One row per key of the section, in the order of `contracts/ledger-observer.md` §15.
pub(crate) static ROWS: &[KeyRow] = &[
    row(BUFFER_BYTES, ValueType::Integer, POSITIVE, AT_OPEN, "AO-2"),
    row(
        BUFFER_RECORDS,
        ValueType::Integer,
        POSITIVE,
        AT_OPEN,
        "AO-2",
    ),
    row(
        BUFFER_TIER1_RATIO,
        ValueType::Decimal,
        KeyRange::Any,
        AT_OPEN,
        "AO-2",
    ),
    row(
        BUFFER_TIER2_RATIO,
        ValueType::Decimal,
        KeyRange::Any,
        AT_OPEN,
        "AO-2",
    ),
    row(
        BUFFER_TIER3_RATIO,
        ValueType::Decimal,
        KeyRange::Any,
        AT_OPEN,
        "AO-2",
    ),
    row(
        BUFFER_HYSTERESIS_RATIO,
        ValueType::Decimal,
        KeyRange::Any,
        AT_OPEN,
        "AO-2",
    ),
    row(
        SPILL_BATCH_BYTES,
        ValueType::Integer,
        POSITIVE,
        AT_OPEN,
        "AO-2",
    ),
    row(
        DUMP_MAX_BYTES,
        ValueType::Integer,
        POSITIVE,
        AT_OPEN,
        "AO-1",
    ),
    row(
        FLUSH_BATCH_RECORDS,
        ValueType::Integer,
        POSITIVE,
        AT_OPEN,
        "AO-2",
    ),
    row(
        FLUSH_BATCH_RECORDS_BUSY,
        ValueType::Integer,
        POSITIVE,
        AT_OPEN,
        "AO-2",
    ),
    row(
        WRITE_ATTEMPTS,
        ValueType::Integer,
        KeyRange::Integer {
            min: 1,
            max: MAX_ATTEMPTS,
        },
        AT_OPEN,
        "AO-1",
    ),
    row(
        WRITE_RETRY_BACKOFF_MS,
        ValueType::List,
        KeyRange::Any,
        AT_OPEN,
        "AO-1",
    ),
    row(
        FLUSH_ATTEMPT_STALL_MS,
        ValueType::DurationMs,
        POSITIVE,
        AT_OPEN,
        "AO-1",
    ),
    row(
        FLUSH_LAG_WARN_MS,
        ValueType::DurationMs,
        POSITIVE,
        WHEN_USED,
        "AO-8",
    ),
    row(
        HEALTH_PROBE_INTERVAL_MS,
        ValueType::DurationMs,
        POSITIVE,
        AT_OPEN,
        "AO-1",
    ),
    row(
        PROBE_INTERVAL_MS,
        ValueType::DurationMs,
        POSITIVE,
        WHEN_USED,
        "AO-8",
    ),
    row(
        PROBE_SAMPLE_KEYS,
        ValueType::Integer,
        POSITIVE,
        WHEN_USED,
        "AO-8",
    ),
    row(
        REQUEST_INDEX_RECORDS,
        ValueType::Integer,
        POSITIVE,
        AT_OPEN,
        "AO-2",
    ),
    row(
        STATUS_GAP_WINDOW_MS,
        ValueType::DurationMs,
        POSITIVE,
        WHEN_USED,
        "AO-8",
    ),
    row(
        TAG_WORKBENCH_PATTERNS,
        ValueType::List,
        KeyRange::Any,
        AT_OPEN,
        "AO-2",
    ),
    row(
        TAG_REOBSERVE_PREFIXES,
        ValueType::List,
        KeyRange::Any,
        AT_OPEN,
        "AO-5",
    ),
];

/// The capture queue's tiers (§7).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CaptureTiers {
    pub tier1_ratio: f64,
    pub tier2_ratio: f64,
    pub tier3_ratio: f64,
    pub hysteresis_ratio: f64,
}

/// The `ledger_observer` section in effect, read and checked whole.
#[derive(Debug, Clone, PartialEq)]
pub struct LedgerObserverConfig {
    pub buffer_bytes: u64,
    pub buffer_records: u64,
    pub tiers: CaptureTiers,
    pub spill_batch_bytes: u64,
    pub dump_max_bytes: u64,
    pub flush_batch_records: u64,
    pub flush_batch_records_busy: u64,
    pub write_attempts: u32,
    pub write_retry_backoff_ms: Vec<u64>,
    pub flush_attempt_stall_ms: u64,
    pub flush_lag_warn_ms: u64,
    pub health_probe_interval_ms: u64,
    pub probe_interval_ms: u64,
    pub probe_sample_keys: u64,
    pub request_index_records: u64,
    pub status_gap_window_ms: u64,
    pub tag_workbench_patterns: Vec<String>,
    pub tag_reobserve_prefixes: Vec<String>,
}

fn invalid(key: &str, problem: ConfigValueProblem) -> ConfigFailure {
    ConfigFailure::invalid_value(key, problem)
}

/// The values of the section's keys, read one by one from the configuration in effect.
struct Reader<'a> {
    value: &'a dyn Fn(&str) -> Option<FactScalar>,
}

impl Reader<'_> {
    fn scalar(&self, key: &str) -> Result<FactScalar, ConfigFailure> {
        (self.value)(key).ok_or_else(|| invalid(key, ConfigValueProblem::WrongType))
    }

    fn count(&self, key: &str) -> Result<u64, ConfigFailure> {
        match self.scalar(key)? {
            FactScalar::Integer(number) => {
                u64::try_from(number).map_err(|_| invalid(key, ConfigValueProblem::OutOfRange))
            }
            _ => Err(invalid(key, ConfigValueProblem::WrongType)),
        }
    }

    fn duration(&self, key: &str) -> Result<u64, ConfigFailure> {
        match self.scalar(key)? {
            FactScalar::DurationMs(ms) => Ok(ms),
            _ => Err(invalid(key, ConfigValueProblem::WrongType)),
        }
    }

    /// A list or decimal key's value, from its canonical JSON text.
    fn json(&self, key: &str) -> Result<Value, ConfigFailure> {
        match self.scalar(key)? {
            FactScalar::String(text) => serde_json::from_str::<Value>(&text)
                .map_err(|_| invalid(key, ConfigValueProblem::WrongType)),
            _ => Err(invalid(key, ConfigValueProblem::WrongType)),
        }
    }

    fn ratio(&self, key: &str) -> Result<f64, ConfigFailure> {
        self.json(key)?
            .as_f64()
            .ok_or_else(|| invalid(key, ConfigValueProblem::WrongType))
    }

    fn waits(&self, key: &str) -> Result<Vec<u64>, ConfigFailure> {
        match self.json(key)? {
            Value::Array(items) => items
                .iter()
                .map(|item| {
                    item.as_u64()
                        .ok_or_else(|| invalid(key, ConfigValueProblem::WrongType))
                })
                .collect(),
            _ => Err(invalid(key, ConfigValueProblem::WrongType)),
        }
    }

    /// A list of key patterns or prefixes: each a non-empty text without space or control
    /// characters.
    fn keys(&self, key: &str) -> Result<Vec<String>, ConfigFailure> {
        let Value::Array(items) = self.json(key)? else {
            return Err(invalid(key, ConfigValueProblem::WrongType));
        };
        items
            .into_iter()
            .map(|item| match item {
                Value::String(text)
                    if !text.is_empty()
                        && !text.chars().any(|character| {
                            character.is_whitespace() || character.is_control()
                        }) =>
                {
                    Ok(text)
                }
                Value::String(_) => Err(invalid(key, ConfigValueProblem::SectionCheck)),
                _ => Err(invalid(key, ConfigValueProblem::WrongType)),
            })
            .collect()
    }
}

impl LedgerObserverConfig {
    /// The section from `value` (the configuration in effect), checked whole (M387 §5.3).
    pub fn parse(value: &dyn Fn(&str) -> Option<FactScalar>) -> Result<Self, ConfigFailure> {
        let reader = Reader { value };
        let tiers = CaptureTiers {
            tier1_ratio: reader.ratio(BUFFER_TIER1_RATIO)?,
            tier2_ratio: reader.ratio(BUFFER_TIER2_RATIO)?,
            tier3_ratio: reader.ratio(BUFFER_TIER3_RATIO)?,
            hysteresis_ratio: reader.ratio(BUFFER_HYSTERESIS_RATIO)?,
        };
        // The tiers rise strictly within (0, 1]; each release line, tier × (1 − hysteresis),
        // lies below its activation line (§7 "configuration check").
        let tier1_rises = tiers.tier1_ratio > 0.0 && tiers.tier1_ratio < tiers.tier2_ratio;
        let tier2_rises = tiers.tier2_ratio < tiers.tier3_ratio;
        let tier3_within = tiers.tier3_ratio <= 1.0;
        let hysteresis_within = tiers.hysteresis_ratio > 0.0 && tiers.hysteresis_ratio < 1.0;
        for (holds, key) in [
            (tier1_rises, BUFFER_TIER1_RATIO),
            (tier2_rises, BUFFER_TIER2_RATIO),
            (tier3_within, BUFFER_TIER3_RATIO),
            (hysteresis_within, BUFFER_HYSTERESIS_RATIO),
        ] {
            if !holds {
                return Err(invalid(key, ConfigValueProblem::SectionCheck));
            }
        }
        let buffer_bytes = reader.count(BUFFER_BYTES)?;
        let spill_batch_bytes = reader.count(SPILL_BATCH_BYTES)?;
        // Tier 3 − tier 2 leaves room for at least one spill batch (§7).
        let spill_room = (tiers.tier3_ratio - tiers.tier2_ratio) * (buffer_bytes as f64);
        if spill_room < spill_batch_bytes as f64 {
            return Err(invalid(SPILL_BATCH_BYTES, ConfigValueProblem::SectionCheck));
        }
        let write_attempts = u32::try_from(reader.count(WRITE_ATTEMPTS)?)
            .map_err(|_| invalid(WRITE_ATTEMPTS, ConfigValueProblem::OutOfRange))?;
        let write_retry_backoff_ms = reader.waits(WRITE_RETRY_BACKOFF_MS)?;
        // One wait before each attempt, the first included (§9.1).
        if usize::try_from(write_attempts).ok() != Some(write_retry_backoff_ms.len()) {
            return Err(invalid(
                WRITE_RETRY_BACKOFF_MS,
                ConfigValueProblem::SectionCheck,
            ));
        }
        Ok(Self {
            buffer_bytes,
            buffer_records: reader.count(BUFFER_RECORDS)?,
            tiers,
            spill_batch_bytes,
            dump_max_bytes: reader.count(DUMP_MAX_BYTES)?,
            flush_batch_records: reader.count(FLUSH_BATCH_RECORDS)?,
            flush_batch_records_busy: reader.count(FLUSH_BATCH_RECORDS_BUSY)?,
            write_attempts,
            write_retry_backoff_ms,
            flush_attempt_stall_ms: reader.duration(FLUSH_ATTEMPT_STALL_MS)?,
            flush_lag_warn_ms: reader.duration(FLUSH_LAG_WARN_MS)?,
            health_probe_interval_ms: reader.duration(HEALTH_PROBE_INTERVAL_MS)?,
            probe_interval_ms: reader.duration(PROBE_INTERVAL_MS)?,
            probe_sample_keys: reader.count(PROBE_SAMPLE_KEYS)?,
            request_index_records: reader.count(REQUEST_INDEX_RECORDS)?,
            status_gap_window_ms: reader.duration(STATUS_GAP_WINDOW_MS)?,
            tag_workbench_patterns: reader.keys(TAG_WORKBENCH_PATTERNS)?,
            tag_reobserve_prefixes: reader.keys(TAG_REOBSERVE_PREFIXES)?,
        })
    }
}
