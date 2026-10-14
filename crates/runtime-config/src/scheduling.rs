// SPDX-License-Identifier: AGPL-3.0-only

//! The `scheduling` section (Workflow #386 SR-1, frozen scheduling model v1.4.2 §9;
//! `contracts/v012-register.md` §5). The Runtime reads the base number of observe-only retries
//! of a timed-out observation from v0.12.0 on (§2.1 item 4). The keys only 0.12.x slices read
//! (`rerun`, `cost_commit`, `blackout`, `feature_lock`, `not_run`) are parsed, checked and
//! defaulted now, so that the one v0.12.0 configuration revision carries them (order §3.5 item
//! 18). Workflow #405: the random task delay (`random_delay`), whose section-level keys are every
//! instance's defaults (the per-instance keys are in the per-instance section), parsed only.
//! Workflow #384 D2-S2: `fact_refresh.recognition_join_timeout_ms`, how long a successful run
//! waits for the readings it handed to recognition, read by the host when a run joins; D2-S4
//! adds the section's other fact refresh keys.
//!
//! The section is read whole and checked whole (M387 §5.3). At start and in `check-config` a
//! host blackout entry that breaks a rule is `blackout_config_invalid` (with `field`, and
//! `blackout_id` when the entry has a usable id); every other broken rule is
//! `config_value_invalid`. Through an instruction both are `config_value_invalid` and the
//! Runtime keeps its configuration. Every default lives in the defaults file.

use crate::codes::ConfigValueProblem;
use crate::effective::ConfigFailure;
use crate::recovery_ladder::{Listed, listed};
use crate::table::{DefaultSource, FilePath, KeyName, KeyRange, KeyRow, Settable, ValueType};
use actingcommand_contract::FactScalar;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

/// The section's name: the first segment of its keys.
pub const SECTION: &str = "scheduling";
/// The base number of observe-only retries of an observation whose sampling timed out (§2.1).
pub const SAMPLE_TIMEOUT_RETRIES: &str = "scheduling.recognition.sample_timeout_retries";
/// The codes that end a run by a condition (§4.1; read at live settlement, SR-2b).
pub const RERUN_CONDITION_CODES: &str = "scheduling.rerun.condition_codes";
/// Condition terminations per occurrence before an ordinary failure (§4.4).
pub const RERUN_MAX_PER_OCCURRENCE: &str = "scheduling.rerun.max_per_occurrence";
/// The highest pacing stage at which an owed rerun runs (§4.3).
pub const RERUN_RESUME_STAGE: &str = "scheduling.rerun.resume_stage";
/// How long the stage must stay at or below the resume stage (§4.3).
pub const RERUN_RESUME_SETTLE_MS: &str = "scheduling.rerun.resume_settle_ms";
/// Whether owed reruns rank first (§4.3).
pub const RERUN_PROMOTE: &str = "scheduling.rerun.promote";
/// Whether a condition termination gives back its counts (§4.2 item 3).
pub const RERUN_REFUND_COUNTS: &str = "scheduling.rerun.refund_counts";
/// When an owed rerun expires (§4.4; one value).
pub const RERUN_EXPIRES: &str = "scheduling.rerun.expires";
/// Whether entry-cost candidates rank first (§4.6).
pub const COST_COMMIT_PROMOTE: &str = "scheduling.cost_commit.promote";
/// The highest pacing stage at which an entry-cost candidate starts (§4.6).
pub const COST_COMMIT_MAX_STAGE: &str = "scheduling.cost_commit.max_stage";
/// The wall-clock margin before an entry-cost step (§4.6).
pub const COST_COMMIT_MARGIN_MS: &str = "scheduling.cost_commit.margin_ms";
/// The host blackout entries (§5.1 b; a list).
pub const BLACKOUT_HOST: &str = "scheduling.blackout.host";
/// The margin of gradual quiet (§5.3).
pub const BLACKOUT_QUIET_MARGIN_MS: &str = "scheduling.blackout.quiet_margin_ms";
/// The hard quiet before a blackout start (§5.3 item 3).
pub const BLACKOUT_HARD_QUIET_MS: &str = "scheduling.blackout.hard_quiet_ms";
/// Whether the safety net stops runs at a blackout start (§5.3 item 5).
pub const BLACKOUT_STOP_IN_FLIGHT_AT_START: &str = "scheduling.blackout.stop_in_flight_at_start";
/// How long a recorded locked feature stays valid (§7.2).
pub const FEATURE_LOCK_LOCKED_VALID_FOR_MS: &str = "scheduling.feature_lock.locked_valid_for_ms";
/// The codes a run ends with on a locked feature (§7.2; a list).
pub const FEATURE_LOCK_CODES: &str = "scheduling.feature_lock.codes";
/// The most rows `ac_diagnose` lists under `not_run` (§8.2).
pub const NOT_RUN_DIAGNOSE_ROWS: &str = "scheduling.not_run.diagnose_rows";
/// Workflow #405: whether the scheduler delays each task it dispatches by a random time. Also the
/// member of an instance's own value (`instance.<instance id>.scheduling.random_delay.enabled`).
pub const RANDOM_DELAY_ENABLED: &str = "scheduling.random_delay.enabled";
/// Workflow #405: the shortest random delay, in milliseconds (also a per-instance member).
pub const RANDOM_DELAY_MIN_MS: &str = "scheduling.random_delay.min_ms";
/// Workflow #405: the longest random delay, in milliseconds (also a per-instance member).
pub const RANDOM_DELAY_MAX_MS: &str = "scheduling.random_delay.max_ms";

/// Workflow #384 D2-S2 (D2 model §3.4, §3.5): how long a successful run waits before
/// `Finalizing` for the readings of the frames it handed to recognition.
pub const FACT_REFRESH_RECOGNITION_JOIN_TIMEOUT_MS: &str =
    "scheduling.fact_refresh.recognition_join_timeout_ms";

/// The longest random delay any key may name: one day (the coordinator's order for Workflow
/// #405, 2026-10-14). Schema, not a threshold.
const MAX_RANDOM_DELAY_MS: i64 = 86_400_000;
/// The largest count an integer key of the section may name: the kernel and the readers count
/// in `u32`. Schema, not a threshold.
const MAX_COUNT: i64 = u32::MAX as i64;
/// The pacing stages a stage key may name: the vocabulary `pacing_stage` of the performance
/// model (s0 normal … s4 severe).
pub const PACING_STAGE_TOKENS: &[&str] = &["s0", "s1", "s2", "s3", "s4"];
/// The words of `rerun.expires`.
const EXPIRES_TOKENS: &[&str] = &["window_end"];
/// A host blackout entry's bounds: the shape of an activity window
/// (`contracts/scheduling/v2/activity.schema.json`, `window`). Schema.
const MAX_UTC_OFFSET_MINUTES: i64 = 840;
const MAX_MINUTE_OF_DAY: i64 = 1_439;
const MAX_WEEKDAY: i64 = 7;
/// A host blackout id's bound: the outcome key `blackout_id` is an `id` (at most 256 bytes, no
/// control character).
const MAX_BLACKOUT_ID_BYTES: usize = 256;

macro_rules! file_struct {
    ($(#[$meta:meta])* $name:ident { $($member:ident),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Default, Deserialize, Serialize)]
        #[serde(deny_unknown_fields)]
        pub struct $name {
            $(
                #[serde(default, skip_serializing_if = "Option::is_none")]
                $member: Option<Value>,
            )+
        }
    };
}

/// The section as the configuration file and the defaults file write it: every member may be
/// left out and none is unknown (M387 §5.3). Each value keeps its JSON here; the key table checks
/// its type and range, the whole-section check its rules.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SchedulingFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    recognition: Option<RecognitionFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rerun: Option<RerunFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cost_commit: Option<CostCommitFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    blackout: Option<BlackoutFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    feature_lock: Option<FeatureLockFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    not_run: Option<NotRunFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    random_delay: Option<RandomDelayFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fact_refresh: Option<FactRefreshFile>,
}

/// Workflow #405: the `scheduling` member of an `instances[]` entry, which holds that instance's
/// own random delay (`instance.<instance id>.scheduling.random_delay.*`).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InstanceSchedulingFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    random_delay: Option<RandomDelayFile>,
}

file_struct!(RecognitionFile {
    sample_timeout_retries
});
file_struct!(RerunFile {
    condition_codes,
    max_per_occurrence,
    resume_stage,
    resume_settle_ms,
    promote,
    refund_counts,
    expires,
});
file_struct!(CostCommitFile {
    promote,
    max_stage,
    margin_ms
});
file_struct!(BlackoutFile {
    host,
    quiet_margin_ms,
    hard_quiet_ms,
    stop_in_flight_at_start,
});
file_struct!(FeatureLockFile {
    locked_valid_for_ms,
    codes
});
file_struct!(NotRunFile { diagnose_rows });
file_struct!(
    /// Workflow #384 D2-S2: the fact refresh keys parsed so far.
    FactRefreshFile { recognition_join_timeout_ms }
);
file_struct!(
    /// Workflow #405: a random delay, section-level or one instance's.
    RandomDelayFile {
        enabled,
        min_ms,
        max_ms
    }
);

const fn row(
    name: &'static str,
    value_type: ValueType,
    range: KeyRange,
    reader: &'static str,
) -> KeyRow {
    KeyRow {
        name: KeyName::Exact(name),
        file_path: FilePath::Key,
        value_type,
        range,
        default: DefaultSource::File,
        // Every key of the section can change while the Runtime runs: each is read where it is
        // used (§9: settings, MCP and CLI read and change them).
        settable: Settable::Yes,
        reader,
        since: "0.12.0",
    }
}

const COUNT: KeyRange = KeyRange::Integer {
    min: 0,
    max: MAX_COUNT,
};
const STAGE: KeyRange = KeyRange::Tokens(PACING_STAGE_TOKENS);
/// Workflow #405: a random delay bound, `0..=86400000` ms.
pub(crate) const DELAY: KeyRange = KeyRange::Integer {
    min: 0,
    max: MAX_RANDOM_DELAY_MS,
};
/// Workflow #384 D2-S2: a join timeout, `0..=1800000` ms: no wait beyond the longest task budget.
const JOIN_TIMEOUT: KeyRange = KeyRange::Integer {
    min: 0,
    max: actingcommand_contract::MAX_CONTAINED_TASK_TIMEOUT_MS as i64,
};

/// One row per key of the section (`contracts/v012-register.md` §5), in the model's order.
pub(crate) static ROWS: &[KeyRow] = &[
    row(SAMPLE_TIMEOUT_RETRIES, ValueType::Integer, COUNT, "SR-1"),
    row(
        RERUN_CONDITION_CODES,
        ValueType::List,
        KeyRange::Any,
        "SR-2b",
    ),
    row(RERUN_MAX_PER_OCCURRENCE, ValueType::Integer, COUNT, "SR-2b"),
    row(RERUN_RESUME_STAGE, ValueType::String, STAGE, "SR-2b"),
    row(
        RERUN_RESUME_SETTLE_MS,
        ValueType::DurationMs,
        KeyRange::Any,
        "SR-2b",
    ),
    row(RERUN_PROMOTE, ValueType::Boolean, KeyRange::Any, "SR-2b"),
    row(
        RERUN_REFUND_COUNTS,
        ValueType::Boolean,
        KeyRange::Any,
        "SR-2b",
    ),
    row(
        RERUN_EXPIRES,
        ValueType::String,
        KeyRange::Tokens(EXPIRES_TOKENS),
        "SR-2b",
    ),
    row(
        COST_COMMIT_PROMOTE,
        ValueType::Boolean,
        KeyRange::Any,
        "SR-10",
    ),
    row(COST_COMMIT_MAX_STAGE, ValueType::String, STAGE, "SR-10"),
    row(
        COST_COMMIT_MARGIN_MS,
        ValueType::DurationMs,
        KeyRange::Any,
        "SR-10",
    ),
    row(BLACKOUT_HOST, ValueType::List, KeyRange::Any, "SR-4a"),
    row(
        BLACKOUT_QUIET_MARGIN_MS,
        ValueType::DurationMs,
        KeyRange::Any,
        "SR-4a",
    ),
    row(
        BLACKOUT_HARD_QUIET_MS,
        ValueType::DurationMs,
        KeyRange::Any,
        "SR-4a",
    ),
    row(
        BLACKOUT_STOP_IN_FLIGHT_AT_START,
        ValueType::Boolean,
        KeyRange::Any,
        "SR-4b",
    ),
    row(
        FEATURE_LOCK_LOCKED_VALID_FOR_MS,
        ValueType::DurationMs,
        KeyRange::Any,
        "SR-5a",
    ),
    row(FEATURE_LOCK_CODES, ValueType::List, KeyRange::Any, "SR-5b"),
    row(NOT_RUN_DIAGNOSE_ROWS, ValueType::Integer, COUNT, "SR-6b"),
    row(
        RANDOM_DELAY_ENABLED,
        ValueType::Boolean,
        KeyRange::Any,
        "#405",
    ),
    row(RANDOM_DELAY_MIN_MS, ValueType::DurationMs, DELAY, "#405"),
    row(RANDOM_DELAY_MAX_MS, ValueType::DurationMs, DELAY, "#405"),
    row(
        FACT_REFRESH_RECOGNITION_JOIN_TIMEOUT_MS,
        ValueType::DurationMs,
        JOIN_TIMEOUT,
        "D2-S2",
    ),
];

/// A pacing stage a stage key names (the vocabulary `pacing_stage`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PacingStage {
    S0,
    S1,
    S2,
    S3,
    S4,
}

impl PacingStage {
    fn parse(token: &str) -> Option<Self> {
        match token {
            "s0" => Some(Self::S0),
            "s1" => Some(Self::S1),
            "s2" => Some(Self::S2),
            "s3" => Some(Self::S3),
            "s4" => Some(Self::S4),
            _ => None,
        }
    }
}

/// When an owed rerun expires (§4.4): the end of the pair's bookkeeping window, the only value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RerunExpiry {
    WindowEnd,
}

/// The `rerun` keys (§4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RerunConfig {
    pub condition_codes: Vec<String>,
    pub max_per_occurrence: u32,
    pub resume_stage: PacingStage,
    pub resume_settle_ms: u64,
    pub promote: bool,
    pub refund_counts: bool,
    pub expires: RerunExpiry,
}

/// The `cost_commit` keys (§4.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CostCommitConfig {
    pub promote: bool,
    pub max_stage: PacingStage,
    pub margin_ms: u64,
}

/// One host blackout entry (§5.1 b; `contracts/scheduling-blackout.md`, "Host blackouts"): the
/// weekdays it starts on (1–7), the offset of its clock from UTC and its start and end minutes
/// of day; an end at or before the start crosses midnight.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostBlackout {
    pub id: String,
    pub weekdays: Vec<u8>,
    pub utc_offset_minutes: i16,
    pub start_minute_of_day: u16,
    pub end_minute_of_day: u16,
}

/// The `blackout` keys (§5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlackoutConfig {
    pub host: Vec<HostBlackout>,
    pub quiet_margin_ms: u64,
    pub hard_quiet_ms: u64,
    pub stop_in_flight_at_start: bool,
}

/// The `feature_lock` keys (§7.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeatureLockConfig {
    pub locked_valid_for_ms: u64,
    pub codes: Vec<String>,
}

/// Workflow #405: a random delay, section-level or one instance's in effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RandomDelay {
    pub enabled: bool,
    pub min_ms: u64,
    pub max_ms: u64,
}

/// The `scheduling` section in effect, read and checked whole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchedulingConfig {
    pub sample_timeout_retries: u32,
    pub rerun: RerunConfig,
    pub cost_commit: CostCommitConfig,
    pub blackout: BlackoutConfig,
    pub feature_lock: FeatureLockConfig,
    pub not_run_diagnose_rows: u32,
    pub random_delay: RandomDelay,
}

fn invalid(key: &str, problem: ConfigValueProblem) -> ConfigFailure {
    ConfigFailure::invalid_value(key, problem)
}

/// The values of the section's keys, read one by one from the configuration in effect.
pub(crate) struct Reader<'a> {
    pub(crate) value: &'a dyn Fn(&str) -> Option<FactScalar>,
}

impl Reader<'_> {
    fn scalar(&self, key: &str) -> Result<FactScalar, ConfigFailure> {
        (self.value)(key).ok_or_else(|| invalid(key, ConfigValueProblem::WrongType))
    }

    pub(crate) fn boolean(&self, key: &str) -> Result<bool, ConfigFailure> {
        match self.scalar(key)? {
            FactScalar::Boolean(flag) => Ok(flag),
            _ => Err(invalid(key, ConfigValueProblem::WrongType)),
        }
    }

    fn count(&self, key: &str) -> Result<u32, ConfigFailure> {
        match self.scalar(key)? {
            FactScalar::Integer(number) => {
                u32::try_from(number).map_err(|_| invalid(key, ConfigValueProblem::OutOfRange))
            }
            _ => Err(invalid(key, ConfigValueProblem::WrongType)),
        }
    }

    pub(crate) fn duration(&self, key: &str) -> Result<u64, ConfigFailure> {
        match self.scalar(key)? {
            FactScalar::DurationMs(ms) => Ok(ms),
            _ => Err(invalid(key, ConfigValueProblem::WrongType)),
        }
    }

    fn stage(&self, key: &str) -> Result<PacingStage, ConfigFailure> {
        match self.scalar(key)? {
            FactScalar::String(token) => PacingStage::parse(&token)
                .ok_or_else(|| invalid(key, ConfigValueProblem::OutOfRange)),
            _ => Err(invalid(key, ConfigValueProblem::WrongType)),
        }
    }

    fn list(&self, key: &str) -> Result<Vec<Value>, ConfigFailure> {
        match self.scalar(key)? {
            FactScalar::String(text) => match serde_json::from_str::<Value>(&text) {
                Ok(Value::Array(items)) => Ok(items),
                _ => Err(invalid(key, ConfigValueProblem::WrongType)),
            },
            _ => Err(invalid(key, ConfigValueProblem::WrongType)),
        }
    }

    /// A list of codes the embedded catalog lists and has not retired: an unlisted code is
    /// `code_unknown`, a retired one `code_retired`.
    fn codes(&self, key: &str) -> Result<Vec<String>, ConfigFailure> {
        self.list(key)?
            .into_iter()
            .map(|entry| {
                let Value::String(code) = entry else {
                    return Err(invalid(key, ConfigValueProblem::WrongType));
                };
                match listed(&code) {
                    Listed::Active => Ok(code),
                    Listed::Retired => Err(invalid(key, ConfigValueProblem::CodeRetired)),
                    Listed::Unlisted => Err(invalid(key, ConfigValueProblem::CodeUnknown)),
                }
            })
            .collect()
    }
}

/// Workflow #405: a random delay from its three keys (`enabled`, `min_ms`, `max_ms`, spelled by
/// `keys`), the bounds in order. `min_ms` above `max_ms` is `config_value_invalid` for the
/// `min_ms` key, `section_check`: the pair is checked as it is in effect, so changing one side
/// alone is checked against the other.
pub(crate) fn random_delay(
    reader: &Reader<'_>,
    keys: [&str; 3],
) -> Result<RandomDelay, ConfigFailure> {
    let [enabled, min_ms, max_ms] = keys;
    let delay = RandomDelay {
        enabled: reader.boolean(enabled)?,
        min_ms: reader.duration(min_ms)?,
        max_ms: reader.duration(max_ms)?,
    };
    // Each bound's own range (`0..=86400000`) is the key table's, checked wherever a value
    // enters (the file layer, the defaults file, an instruction).
    if delay.min_ms > delay.max_ms {
        return Err(invalid(min_ms, ConfigValueProblem::SectionCheck));
    }
    Ok(delay)
}

/// One host blackout entry as written: every member required, none unknown.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HostBlackoutFile {
    id: Value,
    weekdays: Value,
    utc_offset_minutes: Value,
    start_minute_of_day: Value,
    end_minute_of_day: Value,
}

/// The JSON pointer of a member of host blackout entry `index`, or of the entry itself.
fn entry_pointer(index: usize, member: Option<&str>) -> String {
    let entry = format!("/scheduling/blackout/host/{index}");
    match member {
        Some(member) => format!("{entry}/{member}"),
        None => entry,
    }
}

/// An integer within `min..=max`.
fn bounded(value: &Value, min: i64, max: i64) -> Option<i64> {
    value.as_i64().filter(|number| (min..=max).contains(number))
}

/// A usable host blackout id: one to 256 bytes, no control character.
fn blackout_id(value: &Value) -> Option<&str> {
    value.as_str().filter(|id| {
        (1..=MAX_BLACKOUT_ID_BYTES).contains(&id.len()) && !id.chars().any(char::is_control)
    })
}

/// The host blackout entries (`contracts/scheduling-blackout.md`, "Host blackouts"): each an
/// object of `id`, `weekdays`, `utc_offset_minutes`, `start_minute_of_day` and
/// `end_minute_of_day`, ids unique. A broken rule is `blackout_config_invalid` with the member
/// it concerns.
fn host_blackouts(items: Vec<Value>) -> Result<Vec<HostBlackout>, ConfigFailure> {
    let mut ids = BTreeSet::new();
    let mut entries = Vec::with_capacity(items.len());
    for (index, item) in items.into_iter().enumerate() {
        let id = item.get("id").and_then(blackout_id).map(str::to_owned);
        let refused = |member: Option<&str>| {
            ConfigFailure::blackout(BLACKOUT_HOST, entry_pointer(index, member), id.clone())
        };
        let entry = serde_json::from_value::<HostBlackoutFile>(item).map_err(|_| refused(None))?;
        let entry_id = match blackout_id(&entry.id) {
            Some(entry_id) if ids.insert(entry_id.to_owned()) => entry_id.to_owned(),
            _ => return Err(refused(Some("id"))),
        };
        let weekdays = entry
            .weekdays
            .as_array()
            .filter(|days| (1..=MAX_WEEKDAY as usize).contains(&days.len()))
            .and_then(|days| {
                days.iter()
                    .map(|day| bounded(day, 1, MAX_WEEKDAY).and_then(|day| u8::try_from(day).ok()))
                    .collect::<Option<Vec<_>>>()
            })
            .filter(|days| days.iter().collect::<BTreeSet<_>>().len() == days.len())
            .ok_or_else(|| refused(Some("weekdays")))?;
        let utc_offset_minutes = bounded(
            &entry.utc_offset_minutes,
            -MAX_UTC_OFFSET_MINUTES,
            MAX_UTC_OFFSET_MINUTES,
        )
        .and_then(|offset| i16::try_from(offset).ok())
        .ok_or_else(|| refused(Some("utc_offset_minutes")))?;
        let minute = |value: &Value, member: &str| {
            bounded(value, 0, MAX_MINUTE_OF_DAY)
                .and_then(|minute| u16::try_from(minute).ok())
                .ok_or_else(|| refused(Some(member)))
        };
        let start_minute_of_day = minute(&entry.start_minute_of_day, "start_minute_of_day")?;
        let end_minute_of_day = minute(&entry.end_minute_of_day, "end_minute_of_day")?;
        entries.push(HostBlackout {
            id: entry_id,
            weekdays,
            utc_offset_minutes,
            start_minute_of_day,
            end_minute_of_day,
        });
    }
    Ok(entries)
}

impl SchedulingConfig {
    /// The section from `value` (the configuration in effect), checked whole (M387 §5.3).
    pub fn parse(value: &dyn Fn(&str) -> Option<FactScalar>) -> Result<Self, ConfigFailure> {
        let reader = Reader { value };
        let resume_stage = reader.stage(RERUN_RESUME_STAGE)?;
        let expires = match reader.scalar(RERUN_EXPIRES)? {
            FactScalar::String(token) if token == EXPIRES_TOKENS[0] => RerunExpiry::WindowEnd,
            FactScalar::String(_) => {
                return Err(invalid(RERUN_EXPIRES, ConfigValueProblem::OutOfRange));
            }
            _ => return Err(invalid(RERUN_EXPIRES, ConfigValueProblem::WrongType)),
        };
        Ok(Self {
            sample_timeout_retries: reader.count(SAMPLE_TIMEOUT_RETRIES)?,
            rerun: RerunConfig {
                condition_codes: reader.codes(RERUN_CONDITION_CODES)?,
                max_per_occurrence: reader.count(RERUN_MAX_PER_OCCURRENCE)?,
                resume_stage,
                resume_settle_ms: reader.duration(RERUN_RESUME_SETTLE_MS)?,
                promote: reader.boolean(RERUN_PROMOTE)?,
                refund_counts: reader.boolean(RERUN_REFUND_COUNTS)?,
                expires,
            },
            cost_commit: CostCommitConfig {
                promote: reader.boolean(COST_COMMIT_PROMOTE)?,
                max_stage: reader.stage(COST_COMMIT_MAX_STAGE)?,
                margin_ms: reader.duration(COST_COMMIT_MARGIN_MS)?,
            },
            blackout: BlackoutConfig {
                host: host_blackouts(reader.list(BLACKOUT_HOST)?)?,
                quiet_margin_ms: reader.duration(BLACKOUT_QUIET_MARGIN_MS)?,
                hard_quiet_ms: reader.duration(BLACKOUT_HARD_QUIET_MS)?,
                stop_in_flight_at_start: reader.boolean(BLACKOUT_STOP_IN_FLIGHT_AT_START)?,
            },
            feature_lock: FeatureLockConfig {
                locked_valid_for_ms: reader.duration(FEATURE_LOCK_LOCKED_VALID_FOR_MS)?,
                codes: reader.codes(FEATURE_LOCK_CODES)?,
            },
            not_run_diagnose_rows: reader.count(NOT_RUN_DIAGNOSE_ROWS)?,
            random_delay: random_delay(
                &reader,
                [
                    RANDOM_DELAY_ENABLED,
                    RANDOM_DELAY_MIN_MS,
                    RANDOM_DELAY_MAX_MS,
                ],
            )?,
        })
    }
}

/// `scheduling.recognition.sample_timeout_retries` in effect, read alone (the host reads it at
/// each step, §2.1 item 4).
pub(crate) fn sample_timeout_retries(
    value: &dyn Fn(&str) -> Option<FactScalar>,
) -> Result<u32, ConfigFailure> {
    Reader { value }.count(SAMPLE_TIMEOUT_RETRIES)
}

/// Workflow #384 D2-S2: `scheduling.fact_refresh.recognition_join_timeout_ms` in effect, read
/// alone (the host reads it when a run joins its readings).
pub(crate) fn recognition_join_timeout_ms(
    value: &dyn Fn(&str) -> Option<FactScalar>,
) -> Result<u64, ConfigFailure> {
    Reader { value }.duration(FACT_REFRESH_RECOGNITION_JOIN_TIMEOUT_MS)
}
