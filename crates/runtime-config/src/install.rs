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
//! (`wrong_type`, `out_of_range`); a blocked window that breaks its rules, and an unknown member
//! of the section or of one of its objects, `config_value_invalid` (`section_check`).

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
/// Guard `schedule_near`: no affected instance due within this, in milliseconds (INST-2).
pub const GUARD_BEFORE_NEXT_START_MS: &str = "install.guard.before_next_start_ms";
/// Guard `blocked_window`: the windows, in host local time (INST-2).
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

/// The section as the configuration file and the defaults file write it: every member may be
/// left out and none is unknown (M387 §5.3).
// Interface anchor `actingd-config` revision 17 (contracts/component-interfaces.md): a Runtime
// before Workflow #421 INST-1 refuses a file that names the section, because the file denies
// unknown members; a file that does not name it keeps its revision.
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
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StartFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ready_timeout_ms: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    attempts: Option<Value>,
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
    row(GUARD_BLOCKED_WINDOWS, ValueType::List, KeyRange::Any),
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
];

/// One blocked window of `install.guard.blocked_windows`, in host local time, `[from, to)`,
/// wrapping past the end of its day or hour when `to` is less than `from`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockedWindow {
    /// Every day, from and to a minute of the day (`HH:MM`).
    Daily { from_minute: u16, to_minute: u16 },
    /// On the days the window starts on (1 Monday to 7 Sunday), from and to a minute of the day.
    Weekly {
        weekdays: Vec<u8>,
        from_minute: u16,
        to_minute: u16,
    },
    /// Every hour, from and to a minute of the hour.
    Hourly { from_minute: u8, to_minute: u8 },
}

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
    pub guard_blocked_windows: Vec<BlockedWindow>,
    pub guard_adb_timeout_ms: u64,
    pub child_check_config_timeout_ms: u64,
    pub child_ledger_read_timeout_ms: u64,
    pub child_ledger_verify_timeout_ms: u64,
}

/// The section of the configuration document `file` (actingd's configuration as `actingctl`
/// reads it): each key the file's member when it names one, otherwise the shipped defaults
/// file's, checked against its type and range; the blocked windows by their rules. An unknown
/// member of the section or of one of its objects is `config_value_invalid` (`section_check`)
/// for the section; a key the defaults file lacks is `config_value_invalid` for the key.
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
    let (row, windows) = raw(GUARD_BLOCKED_WINDOWS)?;
    parse_value(row, GUARD_BLOCKED_WINDOWS, &windows)?;
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
        guard_blocked_windows: blocked_windows(&windows)?,
        guard_adb_timeout_ms: duration(GUARD_ADB_TIMEOUT_MS)?,
        child_check_config_timeout_ms: duration(CHILD_CHECK_CONFIG_TIMEOUT_MS)?,
        child_ledger_read_timeout_ms: duration(CHILD_LEDGER_READ_TIMEOUT_MS)?,
        child_ledger_verify_timeout_ms: duration(CHILD_LEDGER_VERIFY_TIMEOUT_MS)?,
    })
}

/// The windows of `install.guard.blocked_windows` by their rules; the first that breaks one is
/// `config_value_invalid` (`section_check`) for the key, with the window's JSON pointer.
fn blocked_windows(value: &Value) -> Result<Vec<BlockedWindow>, ConfigFailure> {
    let invalid = |index: usize| {
        ConfigFailure::invalid_value(GUARD_BLOCKED_WINDOWS, ConfigValueProblem::SectionCheck)
            .with_pointer(format!("/{index}"))
    };
    let Some(items) = value.as_array() else {
        return Err(ConfigFailure::invalid_value(
            GUARD_BLOCKED_WINDOWS,
            ConfigValueProblem::WrongType,
        ));
    };
    items
        .iter()
        .enumerate()
        .map(|(index, item)| blocked_window(item).ok_or_else(|| invalid(index)))
        .collect()
}

/// One window, or `None` when it breaks a rule: an object of exactly its kind's members, `from`
/// and `to` of the kind's form and different.
fn blocked_window(item: &Value) -> Option<BlockedWindow> {
    let object = item.as_object()?;
    let kind = object.get("kind")?.as_str()?;
    let allowed: &[&str] = match kind {
        "weekly" => &["kind", "weekdays", "from", "to"],
        "daily" | "hourly" => &["kind", "from", "to"],
        _ => return None,
    };
    if object.keys().any(|name| !allowed.contains(&name.as_str())) {
        return None;
    }
    let window = match kind {
        "hourly" => {
            let minute = |name: &str| {
                object
                    .get(name)?
                    .as_u64()
                    .filter(|minute| *minute <= 59)
                    .and_then(|minute| u8::try_from(minute).ok())
            };
            BlockedWindow::Hourly {
                from_minute: minute("from")?,
                to_minute: minute("to")?,
            }
        }
        _ => {
            let from_minute = minute_of_day(object.get("from")?)?;
            let to_minute = minute_of_day(object.get("to")?)?;
            if kind == "daily" {
                BlockedWindow::Daily {
                    from_minute,
                    to_minute,
                }
            } else {
                let mut weekdays = Vec::new();
                for day in object.get("weekdays")?.as_array()? {
                    let day = day
                        .as_u64()
                        .filter(|day| (1..=7).contains(day))
                        .and_then(|day| u8::try_from(day).ok())?;
                    if weekdays.contains(&day) {
                        return None;
                    }
                    weekdays.push(day);
                }
                if weekdays.is_empty() {
                    return None;
                }
                BlockedWindow::Weekly {
                    weekdays,
                    from_minute,
                    to_minute,
                }
            }
        }
    };
    let (from, to) = match &window {
        BlockedWindow::Daily {
            from_minute,
            to_minute,
        }
        | BlockedWindow::Weekly {
            from_minute,
            to_minute,
            ..
        } => (*from_minute, *to_minute),
        BlockedWindow::Hourly {
            from_minute,
            to_minute,
        } => (u16::from(*from_minute), u16::from(*to_minute)),
    };
    (from != to).then_some(window)
}

/// `HH:MM`, `00:00` to `23:59`, as the minute of the day.
fn minute_of_day(value: &Value) -> Option<u16> {
    let text = value.as_str()?;
    let (hours, minutes) = text.split_once(':')?;
    if hours.len() != 2 || minutes.len() != 2 {
        return None;
    }
    let hours = hours.parse::<u16>().ok().filter(|hours| *hours <= 23)?;
    let minutes = minutes
        .parse::<u16>()
        .ok()
        .filter(|minutes| *minutes <= 59)?;
    Some(hours * 60 + minutes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // Workflow #421 INST-1, test row IT-14 (frozen model INST-M §12; the keys, ranges and
    // defaults of §8): without the section every key takes its default; an explicit value is
    // taken; a value outside its range, an unknown member of the section or of one of its
    // objects, and a window that breaks its rules are refused.
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
            guard_blocked_windows: Vec::new(),
            guard_adb_timeout_ms: 15_000,
            // INST-1b-F fix round 1 (review S1 F4): the deadlines of the installer's children.
            child_check_config_timeout_ms: 900_000,
            child_ledger_read_timeout_ms: 300_000,
            child_ledger_verify_timeout_ms: 1_800_000,
        };
        assert_eq!(settings(&json!({})), Ok(defaults.clone()));
        assert_eq!(settings(&json!({"install": {}})), Ok(defaults.clone()));

        let explicit = settings(&json!({"install": {
            "lock_wait_ms": 1_000,
            "start": {"attempts": 5},
            "guard": {"blocked_windows": [
                {"kind": "weekly", "weekdays": [1, 4], "from": "04:40", "to": "05:40"},
                {"kind": "hourly", "from": 0, "to": 10}
            ]}
        }}))
        .expect("explicit values are taken");
        assert_eq!(explicit.lock_wait_ms, 1_000);
        assert_eq!(explicit.start_attempts, 5);
        assert_eq!(
            explicit.guard_blocked_windows,
            vec![
                BlockedWindow::Weekly {
                    weekdays: vec![1, 4],
                    from_minute: 280,
                    to_minute: 340,
                },
                BlockedWindow::Hourly {
                    from_minute: 0,
                    to_minute: 10,
                },
            ]
        );
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
                json!({"install": {"guard": {"blocked_windows": [
                    {"kind": "daily", "from": "04:40", "to": "05:40", "unknown_member": 1}
                ]}}}),
                GUARD_BLOCKED_WINDOWS,
                ConfigValueProblem::SectionCheck,
            ),
            (
                json!({"install": {"guard": {"blocked_windows": [
                    {"kind": "daily", "from": "24:00", "to": "05:40"}
                ]}}}),
                GUARD_BLOCKED_WINDOWS,
                ConfigValueProblem::SectionCheck,
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
