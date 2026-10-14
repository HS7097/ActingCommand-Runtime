// SPDX-License-Identifier: AGPL-3.0-only

//! The `device_link` section (Workflow #398 RD-2, model-398 v1.1 §4.6; `contracts/
//! v012-register.md` §1.52): how the Runtime watches the wired link of a handset and restores it.
//!
//! - `watch.restart_backoff_ms`: the waits before each restart of the `adb track-devices` watch
//!   after it ended, the last one reused for every later restart;
//! - `watch.restart_limit` and `watch.restart_window_ms`: more restarts than the limit within the
//!   window and the watch is failed (`device_watch_failed`) until a person's self-check;
//! - `settle_ms`: how long a handset stays `device` before its preparation is queued (a plugged
//!   cable often flips between `offline` and `device` first);
//! - `offline_action`: `wait`, or `kick_once`, one `adb reconnect` of that transport per
//!   `offline` episode;
//! - `restore_retry_ms`: the waits before each new preparation after a failed one; when they are
//!   used up the Runtime waits for the next link change or a person's self-check.
//!
//! Every key is read where it is used, so an instruction may change each while the Runtime runs
//! (the watch's at its next restart). The section is read and checked whole (M387 §5.3): both
//! lists are non-empty and the window holds at least the first restart wait; a broken rule is
//! `config_value_invalid` (`section_check`) for the key it concerns. Every default lives in the
//! defaults file; these are starting values that the real-device check may tune (§4.6).

use crate::codes::ConfigValueProblem;
use crate::effective::ConfigFailure;
use crate::table::{DefaultSource, FilePath, KeyName, KeyRange, KeyRow, Settable, ValueType};
use actingcommand_contract::FactScalar;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The section's name: the first segment of its keys.
pub const SECTION: &str = "device_link";
/// The waits before each restart of the link watch, in milliseconds; the last one repeats.
pub const WATCH_RESTART_BACKOFF_MS: &str = "device_link.watch.restart_backoff_ms";
/// The most restarts of the link watch within the window.
pub const WATCH_RESTART_LIMIT: &str = "device_link.watch.restart_limit";
/// The window the restarts are counted in, in milliseconds.
pub const WATCH_RESTART_WINDOW_MS: &str = "device_link.watch.restart_window_ms";
/// How long a handset stays `device` before its preparation is queued, in milliseconds.
pub const SETTLE_MS: &str = "device_link.settle_ms";
/// What the Runtime does while a handset is `offline`: `wait` or `kick_once`.
pub const OFFLINE_ACTION: &str = "device_link.offline_action";
/// The waits before each new preparation after a failed one, in milliseconds; used up, the
/// Runtime waits.
pub const RESTORE_RETRY_MS: &str = "device_link.restore_retry_ms";

/// The words of `offline_action`.
pub const OFFLINE_ACTION_TOKENS: &[&str] = &["wait", "kick_once"];
/// The longest settle wait: one minute (model-398 §4.6). Schema.
const MAX_SETTLE_MS: i64 = 60_000;
/// The restart limit's bounds (model-398 §4.6). Schema.
const MAX_RESTART_LIMIT: i64 = 100;

/// The section as the configuration file and the defaults file write it: every member may be
/// left out and none is unknown (M387 §5.3).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceLinkFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    watch: Option<WatchFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    settle_ms: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    offline_action: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    restore_retry_ms: Option<Value>,
}

/// The `watch` member: the restarts of the link watch.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WatchFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    restart_backoff_ms: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    restart_limit: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    restart_window_ms: Option<Value>,
}

const fn row(name: &'static str, value_type: ValueType, range: KeyRange) -> KeyRow {
    KeyRow {
        name: KeyName::Exact(name),
        file_path: FilePath::Key,
        value_type,
        range,
        default: DefaultSource::File,
        // Read where it is used: the watch's keys at its next restart, the others at the next
        // link change (model-398 §4.6, "运行中可改").
        settable: Settable::Yes,
        reader: "RD-2",
        since: "0.12.0",
    }
}

/// One row per key of the section, in the order of model-398 §4.6.
pub(crate) static ROWS: &[KeyRow] = &[
    row(WATCH_RESTART_BACKOFF_MS, ValueType::List, KeyRange::Any),
    row(
        WATCH_RESTART_LIMIT,
        ValueType::Integer,
        KeyRange::Integer {
            min: 1,
            max: MAX_RESTART_LIMIT,
        },
    ),
    row(
        WATCH_RESTART_WINDOW_MS,
        ValueType::DurationMs,
        KeyRange::Integer {
            min: 1,
            max: i64::MAX,
        },
    ),
    row(
        SETTLE_MS,
        ValueType::DurationMs,
        KeyRange::Integer {
            min: 0,
            max: MAX_SETTLE_MS,
        },
    ),
    row(
        OFFLINE_ACTION,
        ValueType::String,
        KeyRange::Tokens(OFFLINE_ACTION_TOKENS),
    ),
    row(RESTORE_RETRY_MS, ValueType::List, KeyRange::Any),
];

/// What the Runtime does while a handset is `offline` (model-398 §4.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfflineAction {
    /// Wait for the link to change by itself.
    Wait,
    /// One `adb -s <serial> reconnect` of that transport per `offline` episode.
    KickOnce,
}

/// The `device_link` section in effect, read and checked whole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceLinkConfig {
    pub watch_restart_backoff_ms: Vec<u64>,
    pub watch_restart_limit: u32,
    pub watch_restart_window_ms: u64,
    pub settle_ms: u64,
    pub offline_action: OfflineAction,
    pub restore_retry_ms: Vec<u64>,
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

    fn duration(&self, key: &str) -> Result<u64, ConfigFailure> {
        match self.scalar(key)? {
            FactScalar::DurationMs(ms) => Ok(ms),
            _ => Err(invalid(key, ConfigValueProblem::WrongType)),
        }
    }

    /// A non-empty list of milliseconds, from its canonical JSON text.
    fn waits(&self, key: &str) -> Result<Vec<u64>, ConfigFailure> {
        let FactScalar::String(text) = self.scalar(key)? else {
            return Err(invalid(key, ConfigValueProblem::WrongType));
        };
        let Ok(Value::Array(items)) = serde_json::from_str::<Value>(&text) else {
            return Err(invalid(key, ConfigValueProblem::WrongType));
        };
        let waits = items
            .iter()
            .map(|item| {
                item.as_u64()
                    .ok_or_else(|| invalid(key, ConfigValueProblem::WrongType))
            })
            .collect::<Result<Vec<_>, _>>()?;
        if waits.is_empty() {
            return Err(invalid(key, ConfigValueProblem::SectionCheck));
        }
        Ok(waits)
    }
}

impl DeviceLinkConfig {
    /// The section from `value` (the configuration in effect), checked whole (M387 §5.3).
    pub fn parse(value: &dyn Fn(&str) -> Option<FactScalar>) -> Result<Self, ConfigFailure> {
        let reader = Reader { value };
        let watch_restart_backoff_ms = reader.waits(WATCH_RESTART_BACKOFF_MS)?;
        let watch_restart_limit = match reader.scalar(WATCH_RESTART_LIMIT)? {
            FactScalar::Integer(limit) => u32::try_from(limit)
                .ok()
                .filter(|limit| *limit >= 1)
                .ok_or_else(|| invalid(WATCH_RESTART_LIMIT, ConfigValueProblem::OutOfRange))?,
            _ => return Err(invalid(WATCH_RESTART_LIMIT, ConfigValueProblem::WrongType)),
        };
        let watch_restart_window_ms = reader.duration(WATCH_RESTART_WINDOW_MS)?;
        // The window holds at least the first restart wait (model-398 §4.6).
        if watch_restart_backoff_ms
            .first()
            .is_some_and(|first| watch_restart_window_ms < *first)
        {
            return Err(invalid(
                WATCH_RESTART_WINDOW_MS,
                ConfigValueProblem::SectionCheck,
            ));
        }
        let offline_action = match reader.scalar(OFFLINE_ACTION)? {
            FactScalar::String(action) if action == "wait" => OfflineAction::Wait,
            FactScalar::String(action) if action == "kick_once" => OfflineAction::KickOnce,
            FactScalar::String(_) => {
                return Err(invalid(OFFLINE_ACTION, ConfigValueProblem::OutOfRange));
            }
            _ => return Err(invalid(OFFLINE_ACTION, ConfigValueProblem::WrongType)),
        };
        Ok(Self {
            watch_restart_backoff_ms,
            watch_restart_limit,
            watch_restart_window_ms,
            settle_ms: reader.duration(SETTLE_MS)?,
            offline_action,
            restore_retry_ms: reader.waits(RESTORE_RETRY_MS)?,
        })
    }

    /// The wait before restart number `restart` (0 for the first) of the link watch: its list
    /// item, the last one for every later restart.
    pub fn restart_backoff_ms(&self, restart: usize) -> u64 {
        self.watch_restart_backoff_ms
            .get(restart)
            .or_else(|| self.watch_restart_backoff_ms.last())
            .copied()
            .unwrap_or(0)
    }
}
