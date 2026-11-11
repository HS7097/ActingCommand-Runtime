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
//! - `idle.close_app_wake_lead_ms` (default 5 min: a game's cold start through the startup
//!   package, 80 to 141 s in production on 10-27; IDLE-1P, #405, was 60 s) and
//!   `idle.stop_emulator_wake_lead_ms` (default 5 min: the emulator's boot and the startup
//!   package): how long before the next due time the instance is brought back after each action.
//!
//! Each key is also per instance, `instance.<instance id>.idle.<member>` (in the file the
//! `instances[]` entry's `idle` member), defaulting to the section's key of the same name. Every
//! key is settable: the Runtime reads them at every idle check. The whole-section check (the
//! section's and each instance's): each wake lead is shorter than the minimum gap, so an action
//! never wakes the instance at once; and (`EffectiveConfig::check`, per instance) an instance
//! whose action in effect is `stop_emulator` is configured to have its emulator started
//! (`start_emulator`), since the wake starts it through that configured start.
//!
//! IDLE-2 (0.12.54; Alice's words of 11-11: 「根据任务间隔时间自动选择是关闭模拟器还是关闭应用还是回到主页面」;
//! the coordinator's rulings on Workflow #386, comment 6787682270): the value `auto` of
//! `idle.action` takes one tier by the gap to the instance's next task, and two keys, each also per
//! instance, bound the tiers beside `idle.min_gap_ms` (the `close_app` tier's threshold):
//! `idle.return_home_min_gap_ms` (default 10 min), from which the instance rests at home, and
//! `idle.stop_emulator_min_gap_ms` (default 2 h), from which its emulator is stopped; both
//! defaults are starting values to be measured. While the action in effect is `auto`, the
//! whole-section check also holds `return_home_min_gap_ms` ≤ `min_gap_ms` ≤
//! `stop_emulator_min_gap_ms` (`section_check` on the key that breaks it), and `auto` needs no
//! `start_emulator`: the tier stops short of `stop_emulator` instead.

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
/// IDLE-2: under `auto`, the least gap to the next task, in milliseconds, at which the instance
/// rests at home (its game's declared return-home package runs once).
pub const RETURN_HOME_MIN_GAP_MS: &str = "idle.return_home_min_gap_ms";
/// IDLE-2: under `auto`, the least gap to the next task, in milliseconds, at which the instance's
/// emulator is stopped.
pub const STOP_EMULATOR_MIN_GAP_MS: &str = "idle.stop_emulator_min_gap_ms";

/// The values of [`ACTION`]. IDLE-2 adds `auto`.
pub const ACTION_TOKENS: &[&str] = &["none", "close_app", "stop_emulator", "auto"];

/// The bounds of `min_gap_ms` (IDLE-2: and of `return_home_min_gap_ms` and
/// `stop_emulator_min_gap_ms`): one minute to one day (the scheduler's forward horizon). Schema.
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
// Revision 28 (IDLE-2): the value `auto` and the members `return_home_min_gap_ms` and
// `stop_emulator_min_gap_ms`, which a Runtime before IDLE-2 refuses (an unknown value of a closed
// vocabulary, unknown members); a file that names none of them keeps its revision.
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    return_home_min_gap_ms: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stop_emulator_min_gap_ms: Option<Value>,
}

/// One section-level key of the section: its default is the defaults file's.
const fn section_row(name: &'static str, value_type: ValueType, range: KeyRange) -> KeyRow {
    section_row_of(name, value_type, range, "IDLE-1", "0.12.13")
}

/// [`section_row`] read by the slice `reader` since the release `since`.
const fn section_row_of(
    name: &'static str,
    value_type: ValueType,
    range: KeyRange,
    reader: &'static str,
    since: &'static str,
) -> KeyRow {
    KeyRow {
        name: KeyName::Exact(name),
        file_path: FilePath::Key,
        value_type,
        range,
        default: DefaultSource::File,
        // Read at every idle check, so an instruction takes effect at the next one.
        settable: Settable::Yes,
        reader,
        since,
    }
}

/// One instance's own key of the section, named as the section-level key whose value in effect
/// is its default (the per-instance section, `instance.rs`).
pub(crate) const fn instance_row(
    member: &'static str,
    value_type: ValueType,
    range: KeyRange,
) -> KeyRow {
    instance_row_of(member, value_type, range, "IDLE-1", "0.12.13")
}

/// [`instance_row`] read by the slice `reader` since the release `since`.
const fn instance_row_of(
    member: &'static str,
    value_type: ValueType,
    range: KeyRange,
    reader: &'static str,
    since: &'static str,
) -> KeyRow {
    KeyRow {
        name: KeyName::PerInstance(member),
        file_path: FilePath::InstanceMember,
        value_type,
        range,
        default: DefaultSource::Section(member),
        settable: Settable::Yes,
        reader,
        since,
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
    // IDLE-2: the two thresholds of `auto`'s tiers.
    section_row_of(
        RETURN_HOME_MIN_GAP_MS,
        ValueType::DurationMs,
        MIN_GAP_RANGE,
        "IDLE-2",
        "0.12.54",
    ),
    section_row_of(
        STOP_EMULATOR_MIN_GAP_MS,
        ValueType::DurationMs,
        MIN_GAP_RANGE,
        "IDLE-2",
        "0.12.54",
    ),
];

/// The per-instance rows of the section, which `instance.rs` lists among its own.
pub(crate) const INSTANCE_ROWS: [KeyRow; 6] = [
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
    // IDLE-2: the two thresholds of `auto`'s tiers.
    instance_row_of(
        RETURN_HOME_MIN_GAP_MS,
        ValueType::DurationMs,
        MIN_GAP_RANGE,
        "IDLE-2",
        "0.12.54",
    ),
    instance_row_of(
        STOP_EMULATOR_MIN_GAP_MS,
        ValueType::DurationMs,
        MIN_GAP_RANGE,
        "IDLE-2",
        "0.12.54",
    ),
];

/// IDLE-2: the two thresholds of `auto`'s tiers in effect, in milliseconds; `min_gap_ms` is the
/// third (the `close_app` tier's).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutoGaps {
    /// From this gap on the instance rests at home (once per idle stretch).
    pub return_home_min_gap_ms: u64,
    /// From this gap on (or with no next task) the instance's emulator is stopped.
    pub stop_emulator_min_gap_ms: u64,
}

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
    /// IDLE-2: one tier by the gap to the next task, with the thresholds in effect: rest at home,
    /// `close_app` or `stop_emulator`. The Runtime records the tier it took, never this word.
    Auto(AutoGaps),
}

impl IdleAction {
    /// The action's token, as `idle.action` spells it.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::CloseApp => "close_app",
            Self::StopEmulator => "stop_emulator",
            Self::Auto(_) => "auto",
        }
    }

    /// The action `token` names; `auto` with the thresholds `gaps`.
    fn from_token(token: &str, gaps: AutoGaps) -> Option<Self> {
        match token {
            "none" => Some(Self::None),
            "close_app" => Some(Self::CloseApp),
            "stop_emulator" => Some(Self::StopEmulator),
            "auto" => Some(Self::Auto(gaps)),
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
    /// after it; 0 for `none`, which never acts, and for `auto`, which is no tier (IDLE-2: an
    /// instance is brought back after the tier it took, `close_app` or `stop_emulator`).
    pub const fn wake_lead_ms(&self, action: IdleAction) -> u64 {
        match action {
            IdleAction::None | IdleAction::Auto(_) => 0,
            IdleAction::CloseApp => self.close_app_wake_lead_ms,
            IdleAction::StopEmulator => self.stop_emulator_wake_lead_ms,
        }
    }
}

/// The section's members, in the order [`parse_keys`] takes their keys.
const MEMBERS: [&str; 6] = [
    ACTION,
    MIN_GAP_MS,
    CLOSE_APP_WAKE_LEAD_MS,
    STOP_EMULATOR_WAKE_LEAD_MS,
    RETURN_HOME_MIN_GAP_MS,
    STOP_EMULATOR_MIN_GAP_MS,
];

/// The section's keys spelled by `keys` (`action`, `min_gap_ms`, `close_app_wake_lead_ms`,
/// `stop_emulator_wake_lead_ms`, IDLE-2's `return_home_min_gap_ms` and
/// `stop_emulator_min_gap_ms`), each of its type and within its range, each wake lead shorter
/// than the minimum gap (`section_check` on the lead's key), and IDLE-2, while the action is
/// `auto`, `return_home_min_gap_ms` ≤ `min_gap_ms` ≤ `stop_emulator_min_gap_ms` (`section_check`
/// on `return_home_min_gap_ms`, or on `stop_emulator_min_gap_ms`, the key that breaks it);
/// `config_value_invalid` for the first key that is not.
pub(crate) fn parse_keys(
    value: &dyn Fn(&str) -> Option<FactScalar>,
    keys: [&str; 6],
) -> Result<IdleConfig, ConfigFailure> {
    let [
        action_key,
        gap_key,
        close_key,
        stop_key,
        home_gap_key,
        stop_gap_key,
    ] = keys;
    let token = match value(action_key) {
        Some(FactScalar::String(token)) if ACTION_TOKENS.contains(&token.as_str()) => token,
        Some(FactScalar::String(_)) => {
            return Err(ConfigFailure::invalid_value(
                action_key,
                ConfigValueProblem::OutOfRange,
            ));
        }
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
    let gaps = AutoGaps {
        return_home_min_gap_ms: bounded(value, home_gap_key, MIN_GAP_RANGE)?,
        stop_emulator_min_gap_ms: bounded(value, stop_gap_key, MIN_GAP_RANGE)?,
    };
    let action = IdleAction::from_token(&token, gaps)
        .ok_or_else(|| ConfigFailure::invalid_value(action_key, ConfigValueProblem::OutOfRange))?;
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
    // IDLE-2: `auto`'s tiers in order; the thresholds of another action are not checked together.
    if let IdleAction::Auto(gaps) = action {
        if gaps.return_home_min_gap_ms > min_gap_ms {
            return Err(ConfigFailure::invalid_value(
                home_gap_key,
                ConfigValueProblem::SectionCheck,
            ));
        }
        if min_gap_ms > gaps.stop_emulator_min_gap_ms {
            return Err(ConfigFailure::invalid_value(
                stop_gap_key,
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
    parse_keys(value, MEMBERS)
}

/// The keys in effect for the instance whose registry id is `instance_id`: each member the
/// instance's own key (`instance.<instance_id>.idle.<member>`) when the configuration carries it,
/// otherwise the section's; checked whole.
pub(crate) fn instance_idle(
    value: &dyn Fn(&str) -> Option<FactScalar>,
    instance_id: &str,
) -> Result<IdleConfig, ConfigFailure> {
    let keys = MEMBERS.map(|member| {
        let own = crate::table::instance_key(instance_id, member);
        if value(&own).is_some() {
            own
        } else {
            member.to_owned()
        }
    });
    parse_keys(value, keys.each_ref().map(String::as_str))
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
