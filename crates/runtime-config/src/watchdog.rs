// SPDX-License-Identifier: AGPL-3.0-only

//! The `watchdog` section (#411 A66): how the Runtime's watchdog and `actingctl start` /
//! `restart` wait for a start (`contracts/runtime-watchdog.md`, "Start").
//!
//! - `start_window_ms`: how long a start may take before it is `start_timeout`. A start's ledger
//!   open alone may take up to `ledger_observer.open_timeout_ms` (600 s), and a whole start took
//!   4 to 6 minutes on the 10-24 ledger, so the default is 900 000 ms (coordinator ruling,
//!   10-24). The same window bounds which open start an answering owner belongs to, and a
//!   waiter for the watchdog's run lock waits the window plus 20 s.
//!
//! actingd parses and checks the section with the rest of its configuration but never uses it.
//! The watchdog and `actingctl` read it from the configuration file they already read for the
//! state root ([`start_window_ms`]), at every tick or command, so an instruction cannot change
//! it (the Runtime's configuration in effect is not where they read it). Its default lives in
//! the defaults file. A value outside its range is `config_value_invalid` (`out_of_range`).

use crate::codes::{ConfigCode, ConfigValueProblem};
use crate::defaults::shipped_json;
use crate::effective::ConfigFailure;
use crate::resolve::{file_value, member};
use crate::table::{
    DefaultSource, FilePath, KeyName, KeyRange, KeyRow, NotSettable, Settable, ValueType,
};
use crate::value::parse_value;
use actingcommand_contract::FactScalar;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The section's name: the first segment of its keys.
pub const SECTION: &str = "watchdog";
/// How long a start may take before it is `start_timeout`, in milliseconds.
pub const START_WINDOW_MS: &str = "watchdog.start_window_ms";

/// The smallest `start_window_ms`: one minute, the watchdog's tick period.
pub const START_WINDOW_MIN_MS: i64 = 60_000;
/// The largest `start_window_ms`: one hour, the bound of `actingctl start --ready-timeout-s`.
pub const START_WINDOW_MAX_MS: i64 = 3_600_000;
/// The bounds of `start_window_ms`. Schema.
pub const START_WINDOW_RANGE: KeyRange = KeyRange::Integer {
    min: START_WINDOW_MIN_MS,
    max: START_WINDOW_MAX_MS,
};

/// The section as the configuration file and the defaults file write it: every member may be
/// left out and none is unknown (M387 §5.3).
// Interface anchor `actingd-config` revision 11 (contracts/component-interfaces.md): a Runtime
// before #411 A66 refuses a file that names the section, because the file denies unknown
// members; a file that does not name it keeps its revision.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WatchdogFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    start_window_ms: Option<Value>,
}

/// One row per key of the section.
pub(crate) static ROWS: &[KeyRow] = &[KeyRow {
    name: KeyName::Exact(START_WINDOW_MS),
    file_path: FilePath::Key,
    value_type: ValueType::DurationMs,
    range: START_WINDOW_RANGE,
    default: DefaultSource::File,
    // The watchdog and actingctl read it from the configuration file, never from the
    // configuration in effect, so an instruction would change nothing they read.
    settable: Settable::No(NotSettable::ReadAtAssembly),
    reader: "A66",
    since: "0.12.12",
}];

/// `watchdog.start_window_ms` of the configuration document `file` (actingd's configuration as
/// the watchdog and `actingctl` read it): the file's member when it names one, otherwise the
/// shipped defaults file's; within its range. A value outside its type or range is
/// `config_value_invalid`, and a defaults file without the key `config_value_invalid` too.
pub fn start_window_ms(file: &Value) -> Result<u64, ConfigFailure> {
    let row = &ROWS[0];
    let missing = || ConfigFailure::new(ConfigCode::ConfigValueInvalid, START_WINDOW_MS);
    let value = match file_value(row, START_WINDOW_MS, None, file) {
        Some(value) => value,
        None => shipped_json()
            .and_then(|defaults| member(defaults, START_WINDOW_MS))
            .filter(|value| !value.is_null())
            .ok_or_else(missing)?,
    };
    match parse_value(row, START_WINDOW_MS, value)? {
        FactScalar::DurationMs(ms) => Ok(ms),
        _ => Err(
            ConfigFailure::invalid_value(START_WINDOW_MS, ConfigValueProblem::WrongType)
                .with_range(START_WINDOW_RANGE),
        ),
    }
}
