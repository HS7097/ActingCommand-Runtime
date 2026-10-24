// SPDX-License-Identifier: AGPL-3.0-only

//! The `emulator_control` section (#411 A50; `contracts/v012-register.md` §1.158): how the
//! Runtime's emulator control behaves around the vendor's commands.
//!
//! - `restart_exit_wait_ms`: how long a restart (an operator's, or the ladder's emulator-restart
//!   rung) whose Stop's readiness wait expired waits for the emulator status to read the old VM
//!   stopped before it launches; still running at the bound, the restart fails and launches
//!   nothing.
//!
//! Host-wide; read at every restart, so an instruction may change it while the Runtime runs. Its
//! default lives in the defaults file. A value outside its range is `config_value_invalid`
//! (`out_of_range`) at start, in `check-config` and in an instruction.
//!
//! emu-watch (Alice's ruling 35: starting the emulators belongs to the Runtime; review item 5)
//! adds the emulator watch's keys:
//!
//! - `watch_enabled` (default `true`): whether the Runtime polls the emulator status of its
//!   discovery-bound instances;
//! - `watch_interval_ms` (default 60 000): the time between two polls;
//! - `restart_attempts` (default 3) and `restart_window_ms` (default 30 min): at most that many
//!   starts of one instance's stopped emulator by the watch within that window.
//!
//! The watch reads them at every tick, so an instruction may change each while the Runtime runs.

use crate::codes::ConfigValueProblem;
use crate::effective::ConfigFailure;
use crate::table::{DefaultSource, FilePath, KeyName, KeyRange, KeyRow, Settable, ValueType};
use actingcommand_contract::FactScalar;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The section's name: the first segment of its keys.
pub const SECTION: &str = "emulator_control";
/// How long a restart waits for its old VM to exit before it launches, in milliseconds.
pub const RESTART_EXIT_WAIT_MS: &str = "emulator_control.restart_exit_wait_ms";
/// emu-watch: whether the Runtime polls the emulator status of its discovery-bound instances.
pub const WATCH_ENABLED: &str = "emulator_control.watch_enabled";
/// emu-watch: the time between two polls of the emulator status, in milliseconds.
pub const WATCH_INTERVAL_MS: &str = "emulator_control.watch_interval_ms";
/// emu-watch: the most starts of one instance's stopped emulator by the watch within
/// `restart_window_ms`.
pub const RESTART_ATTEMPTS: &str = "emulator_control.restart_attempts";
/// emu-watch: the window `restart_attempts` counts in, in milliseconds.
pub const RESTART_WINDOW_MS: &str = "emulator_control.restart_window_ms";

/// The smallest `restart_exit_wait_ms`: one second.
pub const RESTART_EXIT_WAIT_MIN_MS: i64 = 1_000;
/// The largest `restart_exit_wait_ms`: five minutes. The client's receipt wait of a restart
/// adds it (`runtime-client`, `EMULATOR_RESTART_RESPONSE_TIMEOUT`).
pub const RESTART_EXIT_WAIT_MAX_MS: i64 = 300_000;
/// The bounds of `restart_exit_wait_ms`. Schema.
pub const RESTART_EXIT_WAIT_RANGE: KeyRange = KeyRange::Integer {
    min: RESTART_EXIT_WAIT_MIN_MS,
    max: RESTART_EXIT_WAIT_MAX_MS,
};

/// emu-watch: the bounds of `watch_interval_ms`, five seconds to one hour. Schema.
const WATCH_INTERVAL_RANGE: KeyRange = KeyRange::Integer {
    min: 5_000,
    max: 3_600_000,
};
/// emu-watch: the bounds of `restart_attempts`. Schema.
const RESTART_ATTEMPTS_RANGE: KeyRange = KeyRange::Integer { min: 1, max: 100 };
/// emu-watch: the bounds of `restart_window_ms`, one minute to one day. Schema.
const RESTART_WINDOW_RANGE: KeyRange = KeyRange::Integer {
    min: 60_000,
    max: 86_400_000,
};

/// The section as the configuration file and the defaults file write it: every member may be
/// left out and none is unknown (M387 §5.3).
// Interface anchor `actingd-config` revision 8 (contracts/component-interfaces.md): a Runtime
// before #411 A50 refuses a file that names the section, because the file denies unknown
// members; a file that does not name it keeps its revision.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EmulatorControlFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    restart_exit_wait_ms: Option<Value>,
    // Interface anchor `actingd-config` revision 11 (contracts/component-interfaces.md): a
    // Runtime before emu-watch refuses a file that names one of the watch's members, because the
    // section denies unknown members; a file that names none keeps its revision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    watch_enabled: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    watch_interval_ms: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    restart_attempts: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    restart_window_ms: Option<Value>,
}

/// emu-watch: a key of the watch, read at every tick of the watch, so an instruction takes
/// effect at the next one.
const fn watch_row(name: &'static str, value_type: ValueType, range: KeyRange) -> KeyRow {
    KeyRow {
        name: KeyName::Exact(name),
        file_path: FilePath::Key,
        value_type,
        range,
        default: DefaultSource::File,
        settable: Settable::Yes,
        reader: "EMU-W",
        since: "0.12.12",
    }
}

/// One row per key of the section.
pub(crate) static ROWS: &[KeyRow] = &[
    KeyRow {
        name: KeyName::Exact(RESTART_EXIT_WAIT_MS),
        file_path: FilePath::Key,
        value_type: ValueType::DurationMs,
        range: RESTART_EXIT_WAIT_RANGE,
        default: DefaultSource::File,
        // Read at every restart, so an instruction takes effect at the next one.
        settable: Settable::Yes,
        reader: "A50",
        since: "0.12.9",
    },
    watch_row(WATCH_ENABLED, ValueType::Boolean, KeyRange::Any),
    watch_row(
        WATCH_INTERVAL_MS,
        ValueType::DurationMs,
        WATCH_INTERVAL_RANGE,
    ),
    watch_row(RESTART_ATTEMPTS, ValueType::Integer, RESTART_ATTEMPTS_RANGE),
    watch_row(
        RESTART_WINDOW_MS,
        ValueType::DurationMs,
        RESTART_WINDOW_RANGE,
    ),
];

/// `restart_exit_wait_ms` in effect, within its range; `config_value_invalid` otherwise.
pub(crate) fn restart_exit_wait_ms(
    value: &dyn Fn(&str) -> Option<FactScalar>,
) -> Result<u64, ConfigFailure> {
    let invalid = |problem| {
        ConfigFailure::invalid_value(RESTART_EXIT_WAIT_MS, problem)
            .with_range(RESTART_EXIT_WAIT_RANGE)
    };
    let number = match value(RESTART_EXIT_WAIT_MS) {
        Some(FactScalar::DurationMs(ms)) => i128::from(ms),
        Some(FactScalar::Integer(number)) => i128::from(number),
        _ => return Err(invalid(ConfigValueProblem::WrongType)),
    };
    if !(i128::from(RESTART_EXIT_WAIT_MIN_MS)..=i128::from(RESTART_EXIT_WAIT_MAX_MS))
        .contains(&number)
    {
        return Err(invalid(ConfigValueProblem::OutOfRange));
    }
    u64::try_from(number).map_err(|_| invalid(ConfigValueProblem::OutOfRange))
}

/// emu-watch: the emulator watch's keys of the section in effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmulatorWatchConfig {
    /// Whether the Runtime polls the emulator status of its discovery-bound instances.
    pub enabled: bool,
    /// The time between two polls, in milliseconds (`5000..=3600000`).
    pub interval_ms: u64,
    /// The most starts of one instance's stopped emulator by the watch within `restart_window_ms`
    /// (`1..=100`).
    pub restart_attempts: u32,
    /// The window `restart_attempts` counts in, in milliseconds (`60000..=86400000`).
    pub restart_window_ms: u64,
}

impl EmulatorWatchConfig {
    /// The watch's keys from `value` (the configuration in effect), each of its type and within
    /// its range; `config_value_invalid` for the first key that is not.
    pub(crate) fn parse(value: &dyn Fn(&str) -> Option<FactScalar>) -> Result<Self, ConfigFailure> {
        let enabled = match value(WATCH_ENABLED) {
            Some(FactScalar::Boolean(enabled)) => enabled,
            _ => {
                return Err(ConfigFailure::invalid_value(
                    WATCH_ENABLED,
                    ConfigValueProblem::WrongType,
                ));
            }
        };
        let restart_attempts = bounded(value, RESTART_ATTEMPTS, RESTART_ATTEMPTS_RANGE)?;
        Ok(Self {
            enabled,
            interval_ms: bounded(value, WATCH_INTERVAL_MS, WATCH_INTERVAL_RANGE)?,
            restart_attempts: u32::try_from(restart_attempts).map_err(|_| {
                ConfigFailure::invalid_value(RESTART_ATTEMPTS, ConfigValueProblem::OutOfRange)
                    .with_range(RESTART_ATTEMPTS_RANGE)
            })?,
            restart_window_ms: bounded(value, RESTART_WINDOW_MS, RESTART_WINDOW_RANGE)?,
        })
    }
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
