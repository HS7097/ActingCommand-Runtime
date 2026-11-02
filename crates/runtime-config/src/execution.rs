// SPDX-License-Identifier: AGPL-3.0-only

//! The `execution` section (PERF-B1, Workflow #407 B1; `contracts/v012-register.md` §1.177): the
//! keys the contained-task kernel takes from the configuration, so far its capture cadence
//! (`execution.capture.*`). The register's planned `execution.defaults.*` (the kernel's fallback
//! defaults, PERF §10.4) would join it.
//!
//! - `capture.max_interval_ms`: the largest capture interval a package may declare (its control's
//!   `capture_interval_ms`, a step's `expect_after.interval_ms`, a transition's `interval_ms`); a
//!   run refuses a package that declares a longer one before its first capture;
//! - `capture.backoff_enabled`: whether a step's wait for a page doubles its pause after each
//!   frame that is unchanged from the one before it, returning to the declared interval at the
//!   first changed frame;
//! - `capture.idle_ceiling_ms`: the longest pause that back-off reaches, unless the declared
//!   interval is longer;
//! - `capture.still_limit_ms` (STILL-1, Workflow #407, register §1.249): how long a page wait or a
//!   page-graph confirmation may see unchanged frames, none of them what it accepts, before it
//!   ends; 0, the default, switches it off. Also per instance
//!   (`instance.<instance id>.execution.capture.still_limit_ms`, defaulting to the section's).
//!
//! Host-wide but for that per-instance key. The host hands every key to the kernel with each
//! step's pacing, so an instruction may change each while the Runtime runs: the ceiling takes
//! effect at the next run, the back-off and the still limit at the next step. Every default lives
//! in the defaults file. A value outside its range is `config_value_invalid` (`out_of_range`) at
//! start, in `check-config` and in an instruction; a value of another type is `wrong_type`.

use crate::codes::ConfigValueProblem;
use crate::effective::ConfigFailure;
use crate::table::{
    DefaultSource, FilePath, KeyName, KeyRange, KeyRow, Settable, ValueType, instance_key,
};
use actingcommand_contract::FactScalar;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The section's name: the first segment of its keys.
pub const SECTION: &str = "execution";
/// The largest capture interval a package may declare, in milliseconds.
pub const CAPTURE_MAX_INTERVAL_MS: &str = "execution.capture.max_interval_ms";
/// Whether a step's wait for a page lengthens its pause over unchanged frames.
pub const CAPTURE_BACKOFF_ENABLED: &str = "execution.capture.backoff_enabled";
/// The longest pause the back-off reaches, in milliseconds.
pub const CAPTURE_IDLE_CEILING_MS: &str = "execution.capture.idle_ceiling_ms";
/// STILL-1: how long a wait may see unchanged frames it does not accept, in milliseconds; 0 is
/// off. Also per instance (`instance.<instance id>.execution.capture.still_limit_ms`).
pub const CAPTURE_STILL_LIMIT_MS: &str = "execution.capture.still_limit_ms";

/// The bounds of `capture.max_interval_ms`. Schema: at least the 5 000 ms every package was held
/// to before PERF-B1, so no configuration refuses a package that was admitted before; at most the
/// longest wait a step may declare (`expect_after.timeout_ms`).
pub const CAPTURE_MAX_INTERVAL_RANGE: KeyRange = KeyRange::Integer {
    min: 5_000,
    max: actingcommand_contract::MAX_POSTCONDITION_TIMEOUT_MS as i64,
};
/// The bounds of `capture.idle_ceiling_ms`. Schema: at least the 50 ms capture interval of a
/// package that declares none (below it the back-off could lengthen nothing; the switch turns it
/// off), at most the default ceiling of a declared interval, so a still screen is still looked at
/// once a minute.
pub const CAPTURE_IDLE_CEILING_RANGE: KeyRange = KeyRange::Integer {
    min: 50,
    max: 60_000,
};
/// The least `capture.still_limit_ms` that is on, in milliseconds. Schema: a limit below half a
/// minute would end the waits of ordinary loading screens.
const CAPTURE_STILL_LIMIT_MIN_ON_MS: u64 = 30_000;
/// The bounds of `capture.still_limit_ms` in the key table: 0 (off) to the longest wait a step may
/// declare, above which it could never end one. The table holds one range per key, so
/// [`still_limit_ms`] refuses a value between 0 and [`CAPTURE_STILL_LIMIT_ON_RANGE`].
pub const CAPTURE_STILL_LIMIT_RANGE: KeyRange = KeyRange::Integer {
    min: 0,
    max: actingcommand_contract::MAX_POSTCONDITION_TIMEOUT_MS as i64,
};
/// The bounds of `capture.still_limit_ms` when it is on.
pub const CAPTURE_STILL_LIMIT_ON_RANGE: KeyRange = KeyRange::Integer {
    min: CAPTURE_STILL_LIMIT_MIN_ON_MS as i64,
    max: actingcommand_contract::MAX_POSTCONDITION_TIMEOUT_MS as i64,
};

/// The section as the configuration file and the defaults file write it: every member may be
/// left out and none is unknown (M387 §5.3).
// Interface anchor `actingd-config` revision 11 (contracts/component-interfaces.md): a Runtime
// before PERF-B1 refuses a file that names the section, because the file denies unknown members;
// a file that does not name it keeps its revision.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    capture: Option<ExecutionCaptureFile>,
    // ACT-1, ACT-2 (register §1.250; `action_gap.rs`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    action_gap: Option<crate::action_gap::ActionGapFile>,
}

/// The `execution.capture` member.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionCaptureFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_interval_ms: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    backoff_enabled: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    idle_ceiling_ms: Option<Value>,
    // Interface anchor `actingd-config` revision 24 (contracts/component-interfaces.md; STILL-1):
    // a Runtime before STILL-1 refuses a file that names it, because the member denies unknown
    // members.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    still_limit_ms: Option<Value>,
}

/// STILL-1, ACT-1, ACT-2: the `execution` member of an `instances[]` entry: the instance's own
/// still limit and its own `action_gap` keys.
// Interface anchor `actingd-config` revision 24 (contracts/component-interfaces.md; STILL-1, ACT-1,
// ACT-2): a Runtime before them refuses an entry that names it, because the entry denies unknown
// members.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InstanceExecutionFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    capture: Option<InstanceExecutionCaptureFile>,
    // ACT-1, ACT-2 (register §1.250; `action_gap.rs`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    action_gap: Option<crate::action_gap::ActionGapFile>,
}

/// STILL-1: the `execution.capture` member of an `instances[]` entry.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InstanceExecutionCaptureFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    still_limit_ms: Option<Value>,
}

const fn row(name: &'static str, value_type: ValueType, range: KeyRange) -> KeyRow {
    KeyRow {
        name: KeyName::Exact(name),
        file_path: FilePath::Key,
        value_type,
        range,
        default: DefaultSource::File,
        // Handed to the kernel with each step's pacing, so an instruction takes effect at the
        // next run (the ceiling) or step (the back-off).
        settable: Settable::Yes,
        reader: "PERF-B1",
        since: "0.12.12",
    }
}

/// One row per key of the section.
pub(crate) static ROWS: &[KeyRow] = &[
    row(
        CAPTURE_MAX_INTERVAL_MS,
        ValueType::DurationMs,
        CAPTURE_MAX_INTERVAL_RANGE,
    ),
    row(CAPTURE_BACKOFF_ENABLED, ValueType::Boolean, KeyRange::Any),
    row(
        CAPTURE_IDLE_CEILING_MS,
        ValueType::DurationMs,
        CAPTURE_IDLE_CEILING_RANGE,
    ),
    // STILL-1: handed to the kernel with each step's pacing, so an instruction takes effect at the
    // next step.
    KeyRow {
        name: KeyName::Exact(CAPTURE_STILL_LIMIT_MS),
        file_path: FilePath::Key,
        value_type: ValueType::DurationMs,
        range: CAPTURE_STILL_LIMIT_RANGE,
        default: DefaultSource::File,
        settable: Settable::Yes,
        reader: "STILL-1",
        since: "0.12.37",
    },
];

/// STILL-1: the instance's own still limit; the section's value in effect is its default.
pub(crate) const INSTANCE_STILL_LIMIT_ROW: KeyRow = KeyRow {
    name: KeyName::PerInstance(CAPTURE_STILL_LIMIT_MS),
    file_path: FilePath::InstanceMember,
    value_type: ValueType::DurationMs,
    range: CAPTURE_STILL_LIMIT_RANGE,
    default: DefaultSource::Section(CAPTURE_STILL_LIMIT_MS),
    settable: Settable::Yes,
    reader: "STILL-1",
    since: "0.12.37",
};

/// The `execution.capture` keys in effect: the kernel's capture cadence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureCadenceConfig {
    /// The largest capture interval a package may declare, in milliseconds.
    pub max_interval_ms: u64,
    /// Whether a step's wait for a page lengthens its pause over unchanged frames.
    pub backoff_enabled: bool,
    /// The longest pause the back-off reaches, in milliseconds.
    pub idle_ceiling_ms: u64,
}

/// The milliseconds `key` holds, within `range`; `config_value_invalid` for `key` otherwise.
fn duration(
    value: &dyn Fn(&str) -> Option<FactScalar>,
    key: &str,
    range: KeyRange,
) -> Result<u64, ConfigFailure> {
    let invalid = |problem| ConfigFailure::invalid_value(key, problem).with_range(range);
    let number = match value(key) {
        Some(FactScalar::DurationMs(ms)) => i128::from(ms),
        Some(FactScalar::Integer(number)) => i128::from(number),
        _ => return Err(invalid(ConfigValueProblem::WrongType)),
    };
    let within = match range {
        KeyRange::Integer { min, max } => (i128::from(min)..=i128::from(max)).contains(&number),
        KeyRange::Any | KeyRange::Tokens(_) => true,
    };
    if !within {
        return Err(invalid(ConfigValueProblem::OutOfRange));
    }
    u64::try_from(number).map_err(|_| invalid(ConfigValueProblem::OutOfRange))
}

/// STILL-1: the still limit `key` holds (the section's key or an instance's): 0, or within
/// [`CAPTURE_STILL_LIMIT_ON_RANGE`]; `config_value_invalid` for `key` otherwise.
pub(crate) fn still_limit_ms(
    value: &dyn Fn(&str) -> Option<FactScalar>,
    key: &str,
) -> Result<u64, ConfigFailure> {
    let limit = duration(value, key, CAPTURE_STILL_LIMIT_RANGE)?;
    if limit != 0 && limit < CAPTURE_STILL_LIMIT_MIN_ON_MS {
        return Err(
            ConfigFailure::invalid_value(key, ConfigValueProblem::OutOfRange)
                .with_range(CAPTURE_STILL_LIMIT_ON_RANGE),
        );
    }
    Ok(limit)
}

/// STILL-1: the still limit in effect for the instance whose registry id is `instance_id`: its own
/// `instance.<instance_id>.execution.capture.still_limit_ms` when the configuration carries it,
/// otherwise the section's.
pub(crate) fn instance_still_limit_ms(
    value: &dyn Fn(&str) -> Option<FactScalar>,
    instance_id: &str,
) -> Result<u64, ConfigFailure> {
    let own = instance_key(instance_id, CAPTURE_STILL_LIMIT_MS);
    if value(&own).is_some() {
        still_limit_ms(value, &own)
    } else {
        still_limit_ms(value, CAPTURE_STILL_LIMIT_MS)
    }
}

impl CaptureCadenceConfig {
    /// The keys from `value` (the configuration in effect), each of its type and within its
    /// range; `config_value_invalid` for the first key that is not.
    pub fn parse(value: &dyn Fn(&str) -> Option<FactScalar>) -> Result<Self, ConfigFailure> {
        let max_interval_ms = duration(value, CAPTURE_MAX_INTERVAL_MS, CAPTURE_MAX_INTERVAL_RANGE)?;
        let backoff_enabled = match value(CAPTURE_BACKOFF_ENABLED) {
            Some(FactScalar::Boolean(enabled)) => enabled,
            _ => {
                return Err(ConfigFailure::invalid_value(
                    CAPTURE_BACKOFF_ENABLED,
                    ConfigValueProblem::WrongType,
                ));
            }
        };
        Ok(Self {
            max_interval_ms,
            backoff_enabled,
            idle_ceiling_ms: duration(value, CAPTURE_IDLE_CEILING_MS, CAPTURE_IDLE_CEILING_RANGE)?,
        })
    }
}
