// SPDX-License-Identifier: AGPL-3.0-only

//! The `install` section (Workflow #421 INST-1; `contracts/install-cli.md`, "Configuration
//! keys"; `contracts/runtime-config.md`, "The `install` section"): the waits, attempts, margins,
//! guard values and child deadlines of `actingctl install`, the command-line installer of a
//! single-location root (Alice 10-09: 「绝不硬编码」).
//!
//! actingd parses and checks the section with the rest of its configuration but never uses it.
//! `actingctl install` reads it from the root's configuration file at every command
//! ([`settings`]), never from the configuration in effect, so no key is settable. Its defaults
//! live in the defaults file. A value outside its type or range is `config_value_invalid`
//! (`wrong_type`, `out_of_range`); an unknown member of the section or of one of its objects,
//! `config_value_invalid` (`section_check`). A retired key ([`RETIRED`]) is accepted with no
//! effect (`actingd-config` revision 18).

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
pub const SECTION: &str = "install";
/// How long a command waits for `install\writer.lock`, in milliseconds.
pub const LOCK_WAIT_MS: &str = "install.lock_wait_ms";
/// How long the hold's pauses drain (INST-2), in milliseconds.
pub const STOP_DRAIN_TIMEOUT_MS: &str = "install.stop.drain_timeout_ms";
/// How long the close waits for the Runtime to be gone (INST-2), in milliseconds.
pub const STOP_SHUTDOWN_WAIT_MS: &str = "install.stop.shutdown_wait_ms";
/// The installer start's readiness deadline (INST-2), in milliseconds.
pub const START_READY_TIMEOUT_MS: &str = "install.start.ready_timeout_ms";
/// How many start attempts an error terminal outcome allows (INST-2).
pub const START_ATTEMPTS: &str = "install.start.attempts";
/// Whether a composite update ends with dispatch held (INST-2).
pub const HOLD_AFTER_START: &str = "install.hold_after_start";
/// How long a hot placement waits for idle instances (INST-4), in milliseconds.
pub const HOT_IDLE_WAIT_MS: &str = "install.hot.idle_wait_ms";
/// The rollback gate's margin below the target's open deadline, in milliseconds.
pub const ROLLBACK_OPEN_MARGIN_MS: &str = "install.rollback.open_margin_ms";
/// Guard `memory`: the least available physical memory, in MB (INST-2).
pub const GUARD_MIN_AVAILABLE_MEMORY_MB: &str = "install.guard.min_available_memory_mb";
/// Guard `schedule_near`: what is due within this on a held instance is listed (informational
/// since Alice's decision 75 of 10-27: it never refuses), in milliseconds (INST-2).
pub const GUARD_BEFORE_NEXT_START_MS: &str = "install.guard.before_next_start_ms";
/// Retired in `actingd-config` revision 18 (Alice's decision 75 of 10-27, Runtime v0.12.19):
/// still accepted when it is a list, warned as retired, with no effect.
pub const GUARD_BLOCKED_WINDOWS: &str = "install.guard.blocked_windows";
/// Guard `adb`: each probe's deadline, in milliseconds (INST-2).
pub const GUARD_ADB_TIMEOUT_MS: &str = "install.guard.adb_timeout_ms";
/// INST-1b-F fix round 1 (review S1 F4): each `actingd check-config` the installer runs, its
/// deadline in milliseconds; a child still running then is left running and reported.
pub const CHILD_CHECK_CONFIG_TIMEOUT_MS: &str = "install.child.check_config_timeout_ms";
/// Each `actingledger` read the installer runs (S0, a page of the rollback gate's reader
/// check), its deadline in milliseconds.
pub const CHILD_LEDGER_READ_TIMEOUT_MS: &str = "install.child.ledger_read_timeout_ms";
/// The `actingd ledger-maintenance verify` of an update or rollback, its deadline in
/// milliseconds.
pub const CHILD_LEDGER_VERIFY_TIMEOUT_MS: &str = "install.child.ledger_verify_timeout_ms";
/// Workflow #421 MCP-ENTRY (D3): how often the MCP entry looks at the install, in milliseconds.
pub const MCP_ENTRY_POLL_MS: &str = "install.mcp_entry.poll_ms";
/// How long a change of the install must stay put before the entry asks for a plan.
pub const MCP_ENTRY_SETTLE_MS: &str = "install.mcp_entry.settle_ms";
/// The deadline of a candidate child's verification (and of the plan and a handle count).
pub const MCP_ENTRY_VERIFY_TIMEOUT_MS: &str = "install.mcp_entry.verify_timeout_ms";
/// How long a switch waits for outstanding handles before the status says it waited past it
/// (it keeps waiting).
pub const MCP_ENTRY_DRAIN_MAX_MS: &str = "install.mcp_entry.drain_max_ms";
/// MCP-ENTRY fix round 1 (review M4): the cap of the entry's wait before the next plan after
/// consecutive refusals (the wait doubles from `settle_ms`).
pub const MCP_ENTRY_REFUSED_RETRY_MAX_MS: &str = "install.mcp_entry.refused_retry_max_ms";
/// Workflow #421 INST-2 (#421 deploy design §4, §7; `actingd-config` revision 18, Runtime
/// v0.12.19): how often the installer's start journals a progress sample (the start log's size,
/// the starting process's reads), in milliseconds.
pub const START_SAMPLE_INTERVAL_MS: &str = "install.start.sample_interval_ms";
/// The stall rule's detection: a start whose log has not grown for this long is the caveat
/// `start_stalled`, in milliseconds; nothing is killed.
pub const START_STALL_AFTER_MS: &str = "install.start.stall_after_ms";
/// The stall rule's detection: a start whose process read more than this many bytes before it
/// was ready is the caveat `start_stalled`; nothing is killed.
pub const START_STALL_READ_BYTES: &str = "install.start.stall_read_bytes";
/// INST-2 fix round 1 (review S2 M2; revision 18): how long a start attempt waits for the
/// available memory to reach `install.guard.min_available_memory_mb` before it refuses, in
/// milliseconds (the pages of a Runtime that just stopped are released over time).
pub const START_MEMORY_WAIT_MS: &str = "install.start.memory_wait_ms";
/// INST-2 fix round 1 (review S2 M4; revision 18): how long the hold waits for an instance's
/// queued requests and operator lease to clear when its pause is refused because of them,
/// before it pauses again, in milliseconds.
pub const STOP_BUSY_WAIT_MS: &str = "install.stop.busy_wait_ms";

/// The keys of the section retired in `actingd-config` revision 18 (Alice's decision 75 of
/// 10-27, Runtime v0.12.19): still accepted, of their type, with no effect; every `actingctl
/// install` command that reads the section notes each one the file names.
pub const RETIRED: &[&str] = &[GUARD_BLOCKED_WINDOWS];

/// The section as the configuration file and the defaults file write it: every member may be
/// left out and none is unknown (M387 §5.3).
// Interface anchor `actingd-config` revision 17 (contracts/component-interfaces.md): a Runtime
// before Workflow #421 INST-1 refuses a file that names the section, because the file denies
// unknown members; a file that does not name it keeps its revision. Revision 18 (Workflow #421
// INST-2, Runtime v0.12.19) adds `start.sample_interval_ms`, `start.stall_after_ms` and
// `start.stall_read_bytes` (a build before it refuses a file that names one) and retires
// `guard.blocked_windows`.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InstallFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    lock_wait_ms: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stop: Option<StopFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    start: Option<StartFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    hold_after_start: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    hot: Option<HotFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rollback: Option<RollbackFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    guard: Option<GuardFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    child: Option<ChildFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mcp_entry: Option<McpEntryFile>,
}

/// The MCP entry's tunables (Workflow #421 MCP-ENTRY), read by `actingctl mcp-entry plan`.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct McpEntryFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    poll_ms: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    settle_ms: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    verify_timeout_ms: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    drain_max_ms: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    refused_retry_max_ms: Option<Value>,
}

/// The deadlines of the programs the installer runs (INST-1b-F fix round 1, review S1 F4).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ChildFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    check_config_timeout_ms: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ledger_read_timeout_ms: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ledger_verify_timeout_ms: Option<Value>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StopFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    drain_timeout_ms: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    shutdown_wait_ms: Option<Value>,
    // INST-2 fix round 1 (`actingd-config` revision 18).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    busy_wait_ms: Option<Value>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StartFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ready_timeout_ms: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    attempts: Option<Value>,
    // Workflow #421 INST-2 (#421 deploy design §7; `actingd-config` revision 18): the start's
    // progress samples and the stall rule's detection; the product reports a stall and never
    // kills.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sample_interval_ms: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stall_after_ms: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stall_read_bytes: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    memory_wait_ms: Option<Value>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct HotFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    idle_wait_ms: Option<Value>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RollbackFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    open_margin_ms: Option<Value>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct GuardFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    min_available_memory_mb: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    before_next_start_ms: Option<Value>,
    // Retired in `actingd-config` revision 18: accepted, with no effect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    blocked_windows: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    adb_timeout_ms: Option<Value>,
}

/// A row of the section: read from the file by `actingctl`, never settable.
const fn row(key: &'static str, value_type: ValueType, range: KeyRange) -> KeyRow {
    KeyRow {
        name: KeyName::Exact(key),
        file_path: FilePath::Key,
        value_type,
        range,
        default: DefaultSource::File,
        // `actingctl install` reads the file at every command, never the configuration in
        // effect, so an instruction would change nothing it reads.
        settable: Settable::No(NotSettable::ReadAtAssembly),
        reader: "INST-1",
        since: "0.12.18",
    }
}

/// A row added or changed by Workflow #421 INST-2 in `actingd-config` revision 18 (Runtime
/// v0.12.19).
const fn row_inst2(key: &'static str, value_type: ValueType, range: KeyRange) -> KeyRow {
    KeyRow {
        name: KeyName::Exact(key),
        file_path: FilePath::Key,
        value_type,
        range,
        default: DefaultSource::File,
        settable: Settable::No(NotSettable::ReadAtAssembly),
        reader: "INST-2",
        since: "0.12.19",
    }
}

const fn milliseconds(min: i64, max: i64) -> KeyRange {
    KeyRange::Integer { min, max }
}

/// One row per key of the section.
pub(crate) static ROWS: &[KeyRow] = &[
    row(
        LOCK_WAIT_MS,
        ValueType::DurationMs,
        milliseconds(1_000, 600_000),
    ),
    row(
        STOP_DRAIN_TIMEOUT_MS,
        ValueType::DurationMs,
        milliseconds(1_000, 3_600_000),
    ),
    row(
        STOP_SHUTDOWN_WAIT_MS,
        ValueType::DurationMs,
        milliseconds(10_000, 600_000),
    ),
    row(
        START_READY_TIMEOUT_MS,
        ValueType::DurationMs,
        milliseconds(60_000, 3_600_000),
    ),
    row(
        START_ATTEMPTS,
        ValueType::Integer,
        KeyRange::Integer { min: 1, max: 5 },
    ),
    row(HOLD_AFTER_START, ValueType::Boolean, KeyRange::Any),
    row(
        HOT_IDLE_WAIT_MS,
        ValueType::DurationMs,
        milliseconds(0, 3_600_000),
    ),
    row(
        ROLLBACK_OPEN_MARGIN_MS,
        ValueType::DurationMs,
        milliseconds(0, 600_000),
    ),
    row(
        GUARD_MIN_AVAILABLE_MEMORY_MB,
        ValueType::Integer,
        KeyRange::Integer {
            min: 0,
            max: 1_048_576,
        },
    ),
    row(
        GUARD_BEFORE_NEXT_START_MS,
        ValueType::DurationMs,
        milliseconds(0, 86_400_000),
    ),
    // Retired in revision 18 (Alice's decision 75 of 10-27): accepted, warned, no effect.
    row_inst2(GUARD_BLOCKED_WINDOWS, ValueType::List, KeyRange::Any),
    row(
        GUARD_ADB_TIMEOUT_MS,
        ValueType::DurationMs,
        milliseconds(1_000, 60_000),
    ),
    row(
        CHILD_CHECK_CONFIG_TIMEOUT_MS,
        ValueType::DurationMs,
        milliseconds(10_000, 3_600_000),
    ),
    row(
        CHILD_LEDGER_READ_TIMEOUT_MS,
        ValueType::DurationMs,
        milliseconds(10_000, 3_600_000),
    ),
    row(
        CHILD_LEDGER_VERIFY_TIMEOUT_MS,
        ValueType::DurationMs,
        milliseconds(60_000, 86_400_000),
    ),
    row(
        MCP_ENTRY_POLL_MS,
        ValueType::DurationMs,
        milliseconds(100, 60_000),
    ),
    row(
        MCP_ENTRY_SETTLE_MS,
        ValueType::DurationMs,
        milliseconds(0, 600_000),
    ),
    row(
        MCP_ENTRY_VERIFY_TIMEOUT_MS,
        ValueType::DurationMs,
        milliseconds(1_000, 600_000),
    ),
    row(
        MCP_ENTRY_DRAIN_MAX_MS,
        ValueType::DurationMs,
        milliseconds(0, 86_400_000),
    ),
    row(
        MCP_ENTRY_REFUSED_RETRY_MAX_MS,
        ValueType::DurationMs,
        milliseconds(1_000, 86_400_000),
    ),
    // Workflow #421 INST-2 (#421 deploy design §4, §7): revision 18, Runtime v0.12.19.
    row_inst2(
        START_SAMPLE_INTERVAL_MS,
        ValueType::DurationMs,
        milliseconds(1_000, 600_000),
    ),
    row_inst2(
        START_STALL_AFTER_MS,
        ValueType::DurationMs,
        milliseconds(60_000, 3_600_000),
    ),
    row_inst2(
        START_STALL_READ_BYTES,
        ValueType::Integer,
        KeyRange::Integer {
            min: 0,
            max: 1_099_511_627_776,
        },
    ),
    // INST-2 fix round 1 (review S2 M2, M4): revision 18, Runtime v0.12.19.
    row_inst2(
        START_MEMORY_WAIT_MS,
        ValueType::DurationMs,
        milliseconds(0, 3_600_000),
    ),
    row_inst2(
        STOP_BUSY_WAIT_MS,
        ValueType::DurationMs,
        milliseconds(0, 3_600_000),
    ),
];

/// The section's values, the file's where it names them, else the defaults file's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallSettings {
    pub lock_wait_ms: u64,
    pub stop_drain_timeout_ms: u64,
    pub stop_shutdown_wait_ms: u64,
    pub start_ready_timeout_ms: u64,
    pub start_attempts: u32,
    pub hold_after_start: bool,
    pub hot_idle_wait_ms: u64,
    pub rollback_open_margin_ms: u64,
    pub guard_min_available_memory_mb: u64,
    pub guard_before_next_start_ms: u64,
    pub guard_adb_timeout_ms: u64,
    pub child_check_config_timeout_ms: u64,
    pub child_ledger_read_timeout_ms: u64,
    pub child_ledger_verify_timeout_ms: u64,
    pub mcp_entry_poll_ms: u64,
    pub mcp_entry_settle_ms: u64,
    pub mcp_entry_verify_timeout_ms: u64,
    pub mcp_entry_drain_max_ms: u64,
    pub mcp_entry_refused_retry_max_ms: u64,
    pub start_sample_interval_ms: u64,
    pub start_stall_after_ms: u64,
    pub start_stall_read_bytes: u64,
    pub start_memory_wait_ms: u64,
    pub stop_busy_wait_ms: u64,
    /// The retired keys ([`RETIRED`]) the file names: accepted, with no effect (revision 18).
    pub retired_named: Vec<&'static str>,
}

/// The section of the configuration document `file` (actingd's configuration as `actingctl`
/// reads it): each key the file's member when it names one, otherwise the shipped defaults
/// file's, checked against its type and range; a retired key the file names only against its
/// type, and listed in `retired_named`. An unknown member of the section or of one of its
/// objects is `config_value_invalid` (`section_check`) for the section; a key the defaults file
/// lacks is `config_value_invalid` for the key.
pub fn settings(file: &Value) -> Result<InstallSettings, ConfigFailure> {
    if let Some(section) = file.get(SECTION).filter(|section| !section.is_null()) {
        InstallFile::deserialize(section)
            .map_err(|_| ConfigFailure::invalid_value(SECTION, ConfigValueProblem::SectionCheck))?;
    }
    let raw = |key: &'static str| -> Result<(&'static KeyRow, Value), ConfigFailure> {
        let missing = || ConfigFailure::new(ConfigCode::ConfigValueInvalid, key);
        let row = ROWS
            .iter()
            .find(|row| row.matches(key).is_some())
            .ok_or_else(missing)?;
        let value = match file_value(row, key, None, file) {
            Some(value) => value.clone(),
            None => shipped_json()
                .and_then(|defaults| member(defaults, key))
                .filter(|value| !value.is_null())
                .cloned()
                .ok_or_else(missing)?,
        };
        Ok((row, value))
    };
    let scalar = |key: &'static str| -> Result<FactScalar, ConfigFailure> {
        let (row, value) = raw(key)?;
        parse_value(row, key, &value)
    };
    let duration = |key: &'static str| -> Result<u64, ConfigFailure> {
        match scalar(key)? {
            FactScalar::DurationMs(ms) => Ok(ms),
            _ => Err(ConfigFailure::invalid_value(
                key,
                ConfigValueProblem::WrongType,
            )),
        }
    };
    let integer = |key: &'static str| -> Result<i64, ConfigFailure> {
        match scalar(key)? {
            FactScalar::Integer(number) => Ok(number),
            _ => Err(ConfigFailure::invalid_value(
                key,
                ConfigValueProblem::WrongType,
            )),
        }
    };
    let start_attempts = u32::try_from(integer(START_ATTEMPTS)?).map_err(|_| {
        ConfigFailure::invalid_value(START_ATTEMPTS, ConfigValueProblem::OutOfRange)
    })?;
    let guard_min_available_memory_mb = u64::try_from(integer(GUARD_MIN_AVAILABLE_MEMORY_MB)?)
        .map_err(|_| {
            ConfigFailure::invalid_value(
                GUARD_MIN_AVAILABLE_MEMORY_MB,
                ConfigValueProblem::OutOfRange,
            )
        })?;
    let hold_after_start = match scalar(HOLD_AFTER_START)? {
        FactScalar::Boolean(flag) => flag,
        _ => {
            return Err(ConfigFailure::invalid_value(
                HOLD_AFTER_START,
                ConfigValueProblem::WrongType,
            ));
        }
    };
    // Revision 18 (Alice's decision 75 of 10-27): a retired key is accepted when it has its
    // type, has no effect, and is noted when the file (not the defaults file) names it.
    let mut retired_named = Vec::new();
    for &key in RETIRED {
        let row = ROWS
            .iter()
            .find(|row| row.matches(key).is_some())
            .ok_or_else(|| ConfigFailure::new(ConfigCode::ConfigValueInvalid, key))?;
        if let Some(value) = file_value(row, key, None, file) {
            parse_value(row, key, value)?;
            retired_named.push(key);
        }
    }
    let start_stall_read_bytes = u64::try_from(integer(START_STALL_READ_BYTES)?).map_err(|_| {
        ConfigFailure::invalid_value(START_STALL_READ_BYTES, ConfigValueProblem::OutOfRange)
    })?;
    Ok(InstallSettings {
        lock_wait_ms: duration(LOCK_WAIT_MS)?,
        stop_drain_timeout_ms: duration(STOP_DRAIN_TIMEOUT_MS)?,
        stop_shutdown_wait_ms: duration(STOP_SHUTDOWN_WAIT_MS)?,
        start_ready_timeout_ms: duration(START_READY_TIMEOUT_MS)?,
        start_attempts,
        hold_after_start,
        hot_idle_wait_ms: duration(HOT_IDLE_WAIT_MS)?,
        rollback_open_margin_ms: duration(ROLLBACK_OPEN_MARGIN_MS)?,
        guard_min_available_memory_mb,
        guard_before_next_start_ms: duration(GUARD_BEFORE_NEXT_START_MS)?,
        guard_adb_timeout_ms: duration(GUARD_ADB_TIMEOUT_MS)?,
        child_check_config_timeout_ms: duration(CHILD_CHECK_CONFIG_TIMEOUT_MS)?,
        child_ledger_read_timeout_ms: duration(CHILD_LEDGER_READ_TIMEOUT_MS)?,
        child_ledger_verify_timeout_ms: duration(CHILD_LEDGER_VERIFY_TIMEOUT_MS)?,
        mcp_entry_poll_ms: duration(MCP_ENTRY_POLL_MS)?,
        mcp_entry_settle_ms: duration(MCP_ENTRY_SETTLE_MS)?,
        mcp_entry_verify_timeout_ms: duration(MCP_ENTRY_VERIFY_TIMEOUT_MS)?,
        mcp_entry_drain_max_ms: duration(MCP_ENTRY_DRAIN_MAX_MS)?,
        mcp_entry_refused_retry_max_ms: duration(MCP_ENTRY_REFUSED_RETRY_MAX_MS)?,
        start_sample_interval_ms: duration(START_SAMPLE_INTERVAL_MS)?,
        start_stall_after_ms: duration(START_STALL_AFTER_MS)?,
        start_stall_read_bytes,
        start_memory_wait_ms: duration(START_MEMORY_WAIT_MS)?,
        stop_busy_wait_ms: duration(STOP_BUSY_WAIT_MS)?,
        retired_named,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // Workflow #421 INST-1, test row IT-14 (frozen model INST-M §12; the keys, ranges and
    // defaults of §8): without the section every key takes its default; an explicit value is
    // taken; a value outside its range, and an unknown member of the section or of one of its
    // objects, are refused.
    // Adapted by INST-2 fix round 1 (Alice's decision 75 of 10-27, `actingd-config` revision
    // 18): `install.guard.blocked_windows` is retired, so a file naming it is accepted with no
    // effect and noted in `retired_named`, and the window rules are no longer judged; a value of
    // another type than a list is still refused.
    #[test]
    fn inst1_it_14_the_install_section_defaults_apply_and_bad_members_are_refused() {
        let defaults = InstallSettings {
            lock_wait_ms: 60_000,
            stop_drain_timeout_ms: 300_000,
            stop_shutdown_wait_ms: 60_000,
            start_ready_timeout_ms: 900_000,
            start_attempts: 2,
            hold_after_start: true,
            hot_idle_wait_ms: 600_000,
            rollback_open_margin_ms: 20_000,
            guard_min_available_memory_mb: 10_000,
            guard_before_next_start_ms: 900_000,
            guard_adb_timeout_ms: 15_000,
            // INST-1b-F fix round 1 (review S1 F4): the deadlines of the installer's children.
            child_check_config_timeout_ms: 900_000,
            child_ledger_read_timeout_ms: 300_000,
            child_ledger_verify_timeout_ms: 1_800_000,
            // Workflow #421 MCP-ENTRY: the MCP entry's tunables.
            mcp_entry_poll_ms: 2_000,
            mcp_entry_settle_ms: 10_000,
            mcp_entry_verify_timeout_ms: 60_000,
            mcp_entry_drain_max_ms: 600_000,
            mcp_entry_refused_retry_max_ms: 300_000,
            // Workflow #421 INST-2 (#421 deploy design §4, §7; revision 18): the start's samples
            // and stall detection.
            start_sample_interval_ms: 30_000,
            start_stall_after_ms: 720_000,
            start_stall_read_bytes: 53_687_091_200,
            // INST-2 fix round 1 (review S2 M2, M4; revision 18).
            start_memory_wait_ms: 120_000,
            stop_busy_wait_ms: 300_000,
            retired_named: Vec::new(),
        };
        assert_eq!(settings(&json!({})), Ok(defaults.clone()));
        assert_eq!(settings(&json!({"install": {}})), Ok(defaults.clone()));

        let explicit = settings(&json!({"install": {
            "lock_wait_ms": 1_000,
            "start": {"attempts": 5},
            "guard": {"blocked_windows": [
                {"kind": "weekly", "weekdays": [1, 4], "from": "04:40", "to": "05:40"},
                {"kind": "daily", "from": "24:00", "to": "05:40", "unknown_member": 1}
            ]}
        }}))
        .expect("explicit values are taken and the retired key is accepted");
        assert_eq!(explicit.lock_wait_ms, 1_000);
        assert_eq!(explicit.start_attempts, 5);
        assert_eq!(explicit.retired_named, vec![GUARD_BLOCKED_WINDOWS]);
        assert_eq!(
            explicit.stop_drain_timeout_ms,
            defaults.stop_drain_timeout_ms
        );

        for (file, key, problem) in [
            (
                json!({"install": {"lock_wait_ms": 999}}),
                LOCK_WAIT_MS,
                ConfigValueProblem::OutOfRange,
            ),
            (
                json!({"install": {"start": {"attempts": 6}}}),
                START_ATTEMPTS,
                ConfigValueProblem::OutOfRange,
            ),
            (
                json!({"install": {"guard": {"adb_timeout_ms": 60_001}}}),
                GUARD_ADB_TIMEOUT_MS,
                ConfigValueProblem::OutOfRange,
            ),
            (
                json!({"install": {"hold_after_start": "yes"}}),
                HOLD_AFTER_START,
                ConfigValueProblem::WrongType,
            ),
            (
                json!({"install": {"unknown_member": 1}}),
                SECTION,
                ConfigValueProblem::SectionCheck,
            ),
            (
                json!({"install": {"stop": {"unknown_member": 1}}}),
                SECTION,
                ConfigValueProblem::SectionCheck,
            ),
            (
                json!({"install": {"guard": {"blocked_windows": {"kind": "daily"}}}}),
                GUARD_BLOCKED_WINDOWS,
                ConfigValueProblem::WrongType,
            ),
        ] {
            let failure = settings(&file).expect_err(&file.to_string());
            assert_eq!(
                failure.failure_code(),
                ConfigCode::ConfigValueInvalid,
                "{file}"
            );
            assert_eq!(failure.config_key(), key, "{file}");
            assert_eq!(failure.problem(), Some(problem), "{file}");
        }
    }
}
