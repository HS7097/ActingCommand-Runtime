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
}

/// One row per key of the section.
pub(crate) static ROWS: &[KeyRow] = &[KeyRow {
    name: KeyName::Exact(RESTART_EXIT_WAIT_MS),
    file_path: FilePath::Key,
    value_type: ValueType::DurationMs,
    range: RESTART_EXIT_WAIT_RANGE,
    default: DefaultSource::File,
    // Read at every restart, so an instruction takes effect at the next one.
    settable: Settable::Yes,
    reader: "A50",
    since: "0.12.9",
}];

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
