// SPDX-License-Identifier: AGPL-3.0-only

//! The `startup_discovery` section (Workflow #390 DISC-1, coordinator ruling 2 of wave 18;
//! `contracts/v012-register.md` §1.89): how often a start tries the instance discovery again
//! while it is unavailable, as after a boot where the Runtime starts before the emulator.
//!
//! - `attempts`: the most discovery attempts of one start, the first included; `1` tries once;
//! - `retry_interval_ms`: the wait after each failed attempt but the last, on the Runtime clock.
//!
//! The provider startup reads the section once, before any instruction value is restored, so
//! an instruction may not change it; the configuration file does. Every default lives in the
//! defaults file and covers a normal cold start of the emulator (about three minutes).

use crate::codes::ConfigValueProblem;
use crate::effective::ConfigFailure;
use crate::table::{
    DefaultSource, FilePath, KeyName, KeyRange, KeyRow, NotSettable, Settable, ValueType,
};
use actingcommand_contract::FactScalar;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The section's name: the first segment of its keys.
pub const SECTION: &str = "startup_discovery";
/// The most discovery attempts of one start, the first included.
pub const ATTEMPTS: &str = "startup_discovery.attempts";
/// The wait after each failed discovery attempt but the last, in milliseconds.
pub const RETRY_INTERVAL_MS: &str = "startup_discovery.retry_interval_ms";

/// The bounds of `attempts`. Schema.
// RED PROOF mutation 2: the bounds are not enforced.
const ATTEMPTS_RANGE: KeyRange = KeyRange::Integer { min: 0, max: 1_000 };
/// The bounds of `retry_interval_ms`: one second to five minutes. Schema.
const RETRY_INTERVAL_RANGE: KeyRange = KeyRange::Integer {
    min: 0,
    max: 3_600_000,
};

/// The section as the configuration file and the defaults file write it: every member may be
/// left out and none is unknown (M387 §5.3).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StartupDiscoveryFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    attempts: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    retry_interval_ms: Option<Value>,
}

const fn row(name: &'static str, value_type: ValueType, range: KeyRange) -> KeyRow {
    KeyRow {
        name: KeyName::Exact(name),
        file_path: FilePath::Key,
        value_type,
        range,
        default: DefaultSource::File,
        // Read once by the provider startup, before the instruction values are restored.
        settable: Settable::No(NotSettable::ReadAtAssembly),
        reader: "DISC-1",
        since: "0.12.0",
    }
}

/// One row per key of the section.
pub(crate) static ROWS: &[KeyRow] = &[
    row(ATTEMPTS, ValueType::Integer, ATTEMPTS_RANGE),
    row(
        RETRY_INTERVAL_MS,
        ValueType::DurationMs,
        RETRY_INTERVAL_RANGE,
    ),
];

/// The `startup_discovery` section in effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StartupDiscoveryConfig {
    /// The most discovery attempts of one start, the first included (`1..=100`).
    pub attempts: u32,
    /// The wait after each failed attempt but the last, in milliseconds (`1000..=300000`).
    pub retry_interval_ms: u64,
}

/// The number `key` holds (an integer or a number of milliseconds), within `range`;
/// `config_value_invalid` for `key` otherwise.
fn bounded(
    value: &dyn Fn(&str) -> Option<FactScalar>,
    key: &str,
    range: KeyRange,
) -> Result<u64, ConfigFailure> {
    let invalid = |problem| ConfigFailure::invalid_value(key, problem).with_range(range);
    let number = match value(key) {
        Some(FactScalar::Integer(number)) => i128::from(number),
        Some(FactScalar::DurationMs(ms)) => i128::from(ms),
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

impl StartupDiscoveryConfig {
    /// The section from `value` (the configuration in effect), each key of its type and within
    /// its range; `config_value_invalid` for the first key that is not.
    pub fn parse(value: &dyn Fn(&str) -> Option<FactScalar>) -> Result<Self, ConfigFailure> {
        let attempts = bounded(value, ATTEMPTS, ATTEMPTS_RANGE)?;
        Ok(Self {
            attempts: u32::try_from(attempts).map_err(|_| {
                ConfigFailure::invalid_value(ATTEMPTS, ConfigValueProblem::OutOfRange)
                    .with_range(ATTEMPTS_RANGE)
            })?,
            retry_interval_ms: bounded(value, RETRY_INTERVAL_MS, RETRY_INTERVAL_RANGE)?,
        })
    }
}
