// SPDX-License-Identifier: AGPL-3.0-only

//! IDLE-1 (Alice's ruling 41 of 10-24, and her ruling of 10-25: 「没有任务的时候直接关闭模拟器……
//! 任务结束或者接下来一段时间比如30分钟内没有任务的时候关闭应用程序」, 「这是两个功能，也就是说一个功能下的两种选项」;
//! Workflow #386 comment 6393646823): the `idle` section, what the Runtime does with an instance
//! that has no task soon, and when it brings the instance back.
//!
//! - `idle.action`: `none` (nothing ever happens), `close_app` (the game's application is
//!   stopped inside the emulator; default, ruling 41) or `stop_emulator` (the instance's sessions
//!   are closed and its emulator is stopped).
//! - `idle.min_gap_ms` (default 30 min): the action runs only when the scheduler's next due time
//!   for the instance is at least this far away.
//! - `idle.close_app_wake_lead_ms` (default 60 s) and `idle.stop_emulator_wake_lead_ms` (default
//!   5 min: the emulator's boot and the startup package): how long before the next due time the
//!   instance is brought back after each action.
//!
//! Each key is also per instance, `instance.<instance id>.idle.<member>` (in the file the
//! `instances[]` entry's `idle` member), defaulting to the section's key of the same name. Every
//! key is settable: the Runtime reads them at every idle check. The whole-section check (the
//! section's and each instance's): each wake lead is shorter than the minimum gap, so an action
//! never wakes the instance at once; and (`EffectiveConfig::check`, per instance) an instance
//! whose action in effect is `stop_emulator` is configured to have its emulator started
//! (`start_emulator`), since the wake starts it through that configured start.

use crate::codes::ConfigValueProblem;
use crate::effective::ConfigFailure;
use crate::table::{DefaultSource, FilePath, KeyName, KeyRange, KeyRow, Settable, ValueType};
use actingcommand_contract::FactScalar;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The section's name.
pub const SECTION: &str = "idle";
/// What the Runtime does with an idle instance: one of [`ACTION_TOKENS`].
pub const ACTION: &str = "idle.action";
/// The least time to the next due task, in milliseconds, before the action runs.
pub const MIN_GAP_MS: &str = "idle.min_gap_ms";
/// How long before the next due task an instance whose application was closed is brought back.
pub const CLOSE_APP_WAKE_LEAD_MS: &str = "idle.close_app_wake_lead_ms";
/// How long before the next due task an instance whose emulator was stopped is brought back.
pub const STOP_EMULATOR_WAKE_LEAD_MS: &str = "idle.stop_emulator_wake_lead_ms";

/// The values of [`ACTION`].
pub const ACTION_TOKENS: &[&str] = &["none", "close_app", "stop_emulator"];

/// The bounds of `min_gap_ms`: one minute to one day (the scheduler's forward horizon). Schema.
const MIN_GAP_RANGE: KeyRange = KeyRange::Integer {
    min: 60_000,
    max: 86_400_000,
};
/// The bounds of each wake lead: none to one hour. Schema; the section check keeps each below
/// `min_gap_ms`.
const WAKE_LEAD_RANGE: KeyRange = KeyRange::Integer {
    min: 0,
    max: 3_600_000,
};

/// The section as the configuration file and the defaults file write it, and the `idle` member of
/// an `instances[]` entry: every member may be left out and none is unknown (M387 §5.3).
// Interface anchor `actingd-config` revision 12 (contracts/component-interfaces.md): a Runtime
// before IDLE-1 refuses a file that names the section or an entry's `idle` member, because the
// file and its entries deny unknown members; a file that names neither keeps its revision.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IdleFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    action: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    min_gap_ms: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    close_app_wake_lead_ms: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stop_emulator_wake_lead_ms: Option<Value>,
}

/// One section-level key of the section: its default is the defaults file's.
const fn section_row(name: &'static str, value_type: ValueType, range: KeyRange) -> KeyRow {
    KeyRow {
        name: KeyName::Exact(name),
        file_path: FilePath::Key,
        value_type,
        range,
        default: DefaultSource::File,
        // Read at every idle check, so an instruction takes effect at the next one.
        settable: Settable::Yes,
        reader: "IDLE-1",
        since: "0.12.13",
    }
}

/// One instance's own key of the section, named as the section-level key whose value in effect
/// is its default (the per-instance section, `instance.rs`).
pub(crate) const fn instance_row(
    member: &'static str,
    value_type: ValueType,
    range: KeyRange,
) -> KeyRow {
    KeyRow {
        name: KeyName::PerInstance(member),
        file_path: FilePath::InstanceMember,
        value_type,
        range,
        default: DefaultSource::Section(member),
        settable: Settable::Yes,
        reader: "IDLE-1",
        since: "0.12.13",
    }
}

/// One row per key of the section.
pub(crate) static ROWS: &[KeyRow] = &[
    section_row(ACTION, ValueType::String, KeyRange::Tokens(ACTION_TOKENS)),
    section_row(MIN_GAP_MS, ValueType::DurationMs, MIN_GAP_RANGE),
    section_row(
        CLOSE_APP_WAKE_LEAD_MS,
        ValueType::DurationMs,
        WAKE_LEAD_RANGE,
    ),
    section_row(
        STOP_EMULATOR_WAKE_LEAD_MS,
        ValueType::DurationMs,
        WAKE_LEAD_RANGE,
    ),
];

/// The per-instance rows of the section, which `instance.rs` lists among its own.
pub(crate) const INSTANCE_ROWS: [KeyRow; 4] = [
    instance_row(ACTION, ValueType::String, KeyRange::Tokens(ACTION_TOKENS)),
    instance_row(MIN_GAP_MS, ValueType::DurationMs, MIN_GAP_RANGE),
    instance_row(
        CLOSE_APP_WAKE_LEAD_MS,
        ValueType::DurationMs,
        WAKE_LEAD_RANGE,
    ),
    instance_row(
        STOP_EMULATOR_WAKE_LEAD_MS,
        ValueType::DurationMs,
        WAKE_LEAD_RANGE,
    ),
];

/// What the Runtime does with an idle instance (`idle.action`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdleAction {
    /// Nothing ever happens.
    None,
    /// The game's application is stopped inside the emulator; the startup package launches it
    /// again.
    CloseApp,
    /// The instance's sessions are closed and its emulator is stopped; the configured emulator
    /// start and the startup package bring it back.
    StopEmulator,
}

impl IdleAction {
    /// The action's token, as `idle.action` spells it.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::CloseApp => "close_app",
            Self::StopEmulator => "stop_emulator",
        }
    }

    fn from_token(token: &str) -> Option<Self> {
        match token {
            "none" => Some(Self::None),
            "close_app" => Some(Self::CloseApp),
            "stop_emulator" => Some(Self::StopEmulator),
            _ => None,
        }
    }
}

/// The section's keys in effect, the section's or, from `EffectiveConfig::idle`, one instance's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdleConfig {
    pub action: IdleAction,
    /// The least time to the next due task before the action runs, in milliseconds.
    pub min_gap_ms: u64,
    /// How long before the next due task a closed application is launched again, in ms.
    pub close_app_wake_lead_ms: u64,
    /// How long before the next due task a stopped emulator is started again, in ms.
    pub stop_emulator_wake_lead_ms: u64,
}

impl IdleConfig {
    /// The wake lead of `action`: how long before the next due task the instance is brought back
    /// after it; 0 for `none`, which never acts.
    pub const fn wake_lead_ms(&self, action: IdleAction) -> u64 {
        match action {
            IdleAction::None => 0,
            IdleAction::CloseApp => self.close_app_wake_lead_ms,
            IdleAction::StopEmulator => self.stop_emulator_wake_lead_ms,
        }
    }
}

/// The section's keys spelled by `keys` (`action`, `min_gap_ms`, `close_app_wake_lead_ms`,
/// `stop_emulator_wake_lead_ms`), each of its type and within its range, and each wake lead
/// shorter than the minimum gap (`section_check` on the lead's key); `config_value_invalid` for
/// the first key that is not.
pub(crate) fn parse_keys(
    value: &dyn Fn(&str) -> Option<FactScalar>,
    keys: [&str; 4],
) -> Result<IdleConfig, ConfigFailure> {
    let [action_key, gap_key, close_key, stop_key] = keys;
    let action = match value(action_key) {
        Some(FactScalar::String(token)) => IdleAction::from_token(&token).ok_or_else(|| {
            ConfigFailure::invalid_value(action_key, ConfigValueProblem::OutOfRange)
        })?,
        _ => {
            return Err(ConfigFailure::invalid_value(
                action_key,
                ConfigValueProblem::WrongType,
            ));
        }
    };
    let min_gap_ms = bounded(value, gap_key, MIN_GAP_RANGE)?;
    let close_app_wake_lead_ms = bounded(value, close_key, WAKE_LEAD_RANGE)?;
    let stop_emulator_wake_lead_ms = bounded(value, stop_key, WAKE_LEAD_RANGE)?;
    for (key, lead_ms) in [
        (close_key, close_app_wake_lead_ms),
        (stop_key, stop_emulator_wake_lead_ms),
    ] {
        if lead_ms >= min_gap_ms {
            return Err(ConfigFailure::invalid_value(
                key,
                ConfigValueProblem::SectionCheck,
            ));
        }
    }
    Ok(IdleConfig {
        action,
        min_gap_ms,
        close_app_wake_lead_ms,
        stop_emulator_wake_lead_ms,
    })
}

/// The section's keys in effect, checked whole.
pub(crate) fn section(
    value: &dyn Fn(&str) -> Option<FactScalar>,
) -> Result<IdleConfig, ConfigFailure> {
    parse_keys(
        value,
        [
            ACTION,
            MIN_GAP_MS,
            CLOSE_APP_WAKE_LEAD_MS,
            STOP_EMULATOR_WAKE_LEAD_MS,
        ],
    )
}

/// The keys in effect for the instance whose registry id is `instance_id`: each member the
/// instance's own key (`instance.<instance_id>.idle.<member>`) when the configuration carries it,
/// otherwise the section's; checked whole.
pub(crate) fn instance_idle(
    value: &dyn Fn(&str) -> Option<FactScalar>,
    instance_id: &str,
) -> Result<IdleConfig, ConfigFailure> {
    let keys = [
        ACTION,
        MIN_GAP_MS,
        CLOSE_APP_WAKE_LEAD_MS,
        STOP_EMULATOR_WAKE_LEAD_MS,
    ]
    .map(|member| {
        let own = crate::table::instance_key(instance_id, member);
        if value(&own).is_some() {
            own
        } else {
            member.to_owned()
        }
    });
    let [action, gap, close, stop] = &keys;
    parse_keys(
        value,
        [action.as_str(), gap.as_str(), close.as_str(), stop.as_str()],
    )
}

/// The number of milliseconds `key` holds, within `range`; `config_value_invalid` for `key`
/// otherwise.
fn bounded(
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
