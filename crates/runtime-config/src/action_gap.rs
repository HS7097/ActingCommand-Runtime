// SPDX-License-Identifier: AGPL-3.0-only

//! ACT-1 and ACT-2 (Workflow #407, register §1.250; `contracts/runtime-config.md`, "The
//! `execution` section"): the `execution.action_gap` keys, how a step goes on after its input and
//! what lies between two actions.
//!
//! - `mode`: `declared` (the default) is the order before these keys, byte for byte: the input,
//!   the blind `post_delay_ms`, the confirmation. `settled` reads frames from the first frame after
//!   the input, and `post_delay_ms` only limits that reading.
//! - `kind`, `fixed_ms`, `random_min_ms`, `random_max_ms`: the gap a step of a `settled` instance
//!   waits before the frame that decides its input: a fixed one, one drawn per step, or
//!   (`performance`) none, leaving the pacing curve's pre-input wait, which stays under every kind.
//! - `poll_ms`: the pause between two frames of that reading.
//! - `settle_frames`, `settle_change_max_milli`: how many consecutive destination frames, unchanged
//!   by at most that many milli, the reading takes; 1 goes on the first.
//! - `settled_max_stage`: the highest pacing stage at which a step reads that way.
//! - `fallback_hold_ms`: how long an operation that failed on that reading stays on `declared`.
//!
//! Each key is settable, read at each step, and also per instance
//! (`instance.<instance id>.execution.action_gap.<member>`, in the file the entry's
//! `execution.action_gap` member). An instance's own key is reported only when the file names it or
//! an instruction sets it; otherwise the section's is in effect. The whole-section check:
//! `random_min_ms` at most `random_max_ms`, each as it is in effect.

use crate::codes::ConfigValueProblem;
use crate::effective::ConfigFailure;
use crate::scheduling::PACING_STAGE_TOKENS;
use crate::table::{
    DefaultSource, FilePath, KeyName, KeyRange, KeyRow, Settable, ValueType, instance_key,
};
use actingcommand_contract::{FactScalar, PerformancePacingStage};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// How a step goes on after its input: one of [`MODE_TOKENS`].
pub const MODE: &str = "execution.action_gap.mode";
/// What a `settled` step adds before its input: one of [`KIND_TOKENS`].
pub const KIND: &str = "execution.action_gap.kind";
/// The gap of the kind `fixed`, in milliseconds.
pub const FIXED_MS: &str = "execution.action_gap.fixed_ms";
/// The least gap of the kind `random`, in milliseconds.
pub const RANDOM_MIN_MS: &str = "execution.action_gap.random_min_ms";
/// The largest gap of the kind `random`, in milliseconds.
pub const RANDOM_MAX_MS: &str = "execution.action_gap.random_max_ms";
/// The pause between two frames of a `settled` step's reading, in milliseconds.
pub const POLL_MS: &str = "execution.action_gap.poll_ms";
/// How many consecutive destination frames a `settled` step takes.
pub const SETTLE_FRAMES: &str = "execution.action_gap.settle_frames";
/// The largest frame change, in milli, between two of those frames.
pub const SETTLE_CHANGE_MAX_MILLI: &str = "execution.action_gap.settle_change_max_milli";
/// The highest pacing stage at which a step reads frames after its input.
pub const SETTLED_MAX_STAGE: &str = "execution.action_gap.settled_max_stage";
/// How long an operation that failed on that reading stays on `declared`, in milliseconds.
pub const FALLBACK_HOLD_MS: &str = "execution.action_gap.fallback_hold_ms";

/// The values of [`MODE`].
pub const MODE_TOKENS: &[&str] = &["declared", "settled"];
/// The values of [`KIND`].
pub const KIND_TOKENS: &[&str] = &["fixed", "random", "performance"];

/// The bounds of a gap: none to one minute. Schema.
const GAP_RANGE: KeyRange = KeyRange::Integer {
    min: 0,
    max: 60_000,
};
/// The bounds of `poll_ms`: none to one second. Schema.
const POLL_RANGE: KeyRange = KeyRange::Integer { min: 0, max: 1_000 };
/// The bounds of `settle_frames`. Schema.
const SETTLE_FRAMES_RANGE: KeyRange = KeyRange::Integer { min: 1, max: 8 };
/// The bounds of `settle_change_max_milli`: the frame change of two frames is a mean over the
/// cells of a colour distance, in milli, and every colour differs by less than this. Schema.
const SETTLE_CHANGE_RANGE: KeyRange = KeyRange::Integer {
    min: 0,
    max: 1_000_000,
};
/// The bounds of `fallback_hold_ms`: none to one day. Schema.
const FALLBACK_HOLD_RANGE: KeyRange = KeyRange::Integer {
    min: 0,
    max: 86_400_000,
};

/// The `execution.action_gap` member of the section and of an instance's `execution` member: every
/// member may be left out and none is unknown (M387 §5.3).
// Interface anchor `actingd-config` revision 24 (contracts/component-interfaces.md): a Runtime
// before ACT-1 refuses a file that names it, because the section and the entries deny unknown
// members; a file that does not name it keeps its revision.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActionGapFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mode: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    kind: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fixed_ms: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    random_min_ms: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    random_max_ms: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    poll_ms: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    settle_frames: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    settle_change_max_milli: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    settled_max_stage: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fallback_hold_ms: Option<Value>,
}

const fn section_row(name: &'static str, value_type: ValueType, range: KeyRange) -> KeyRow {
    KeyRow {
        name: KeyName::Exact(name),
        file_path: FilePath::Key,
        value_type,
        range,
        default: DefaultSource::File,
        // Read at each step, so an instruction takes effect at the next one.
        settable: Settable::Yes,
        reader: "ACT-1",
        since: "0.12.37",
    }
}

/// One instance's own key, named as the section-level key it overrides. No default: an instance
/// that names none reads the section's, and reports no row of its own.
const fn instance_row(member: &'static str, value_type: ValueType, range: KeyRange) -> KeyRow {
    KeyRow {
        name: KeyName::PerInstance(member),
        file_path: FilePath::InstanceMember,
        value_type,
        range,
        default: DefaultSource::None,
        settable: Settable::Yes,
        reader: "ACT-1",
        since: "0.12.37",
    }
}

/// One row per section-level key.
pub(crate) static ROWS: &[KeyRow] = &[
    section_row(MODE, ValueType::String, KeyRange::Tokens(MODE_TOKENS)),
    section_row(KIND, ValueType::String, KeyRange::Tokens(KIND_TOKENS)),
    section_row(FIXED_MS, ValueType::DurationMs, GAP_RANGE),
    section_row(RANDOM_MIN_MS, ValueType::DurationMs, GAP_RANGE),
    section_row(RANDOM_MAX_MS, ValueType::DurationMs, GAP_RANGE),
    section_row(POLL_MS, ValueType::DurationMs, POLL_RANGE),
    section_row(SETTLE_FRAMES, ValueType::Integer, SETTLE_FRAMES_RANGE),
    section_row(
        SETTLE_CHANGE_MAX_MILLI,
        ValueType::Integer,
        SETTLE_CHANGE_RANGE,
    ),
    section_row(
        SETTLED_MAX_STAGE,
        ValueType::String,
        KeyRange::Tokens(PACING_STAGE_TOKENS),
    ),
    section_row(FALLBACK_HOLD_MS, ValueType::DurationMs, FALLBACK_HOLD_RANGE),
];

/// One row per instance's own key, after the per-instance section's other rows.
pub(crate) static INSTANCE_ROWS: &[KeyRow] = &[
    instance_row(MODE, ValueType::String, KeyRange::Tokens(MODE_TOKENS)),
    instance_row(KIND, ValueType::String, KeyRange::Tokens(KIND_TOKENS)),
    instance_row(FIXED_MS, ValueType::DurationMs, GAP_RANGE),
    instance_row(RANDOM_MIN_MS, ValueType::DurationMs, GAP_RANGE),
    instance_row(RANDOM_MAX_MS, ValueType::DurationMs, GAP_RANGE),
    instance_row(POLL_MS, ValueType::DurationMs, POLL_RANGE),
    instance_row(SETTLE_FRAMES, ValueType::Integer, SETTLE_FRAMES_RANGE),
    instance_row(
        SETTLE_CHANGE_MAX_MILLI,
        ValueType::Integer,
        SETTLE_CHANGE_RANGE,
    ),
    instance_row(
        SETTLED_MAX_STAGE,
        ValueType::String,
        KeyRange::Tokens(PACING_STAGE_TOKENS),
    ),
    instance_row(FALLBACK_HOLD_MS, ValueType::DurationMs, FALLBACK_HOLD_RANGE),
];

/// How a step goes on after its input (`execution.action_gap.mode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionGapMode {
    /// The input, the blind `post_delay_ms`, the confirmation; no gap is added.
    Declared,
    /// Frames from the first frame after the input, `post_delay_ms` as their limit; the gap comes
    /// before the frame that decides the input.
    Settled,
}

/// What a `settled` step adds before its input (`execution.action_gap.kind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionGapKind {
    /// `fixed_ms`.
    Fixed,
    /// A value of `random_min_ms..=random_max_ms`, drawn per step.
    Random,
    /// Nothing beyond the pacing curve's term.
    Performance,
}

/// The keys in effect, the section's or, from `EffectiveConfig::action_gap`, one instance's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActionGapConfig {
    pub mode: ActionGapMode,
    pub kind: ActionGapKind,
    pub fixed_ms: u64,
    pub random_min_ms: u64,
    pub random_max_ms: u64,
    pub poll_ms: u64,
    pub settle_frames: u32,
    pub settle_change_max_milli: u32,
    pub settled_max_stage: PerformancePacingStage,
    pub fallback_hold_ms: u64,
}

impl ActionGapConfig {
    /// Whether a step's gap is drawn: the kind `random` of a `settled` instance.
    pub const fn draws(&self) -> bool {
        matches!(self.mode, ActionGapMode::Settled) && matches!(self.kind, ActionGapKind::Random)
    }

    /// The gap of one step, in milliseconds, from `seed` when it is drawn: none under `declared`
    /// and for the kind `performance`.
    pub const fn gap_ms(&self, seed: u64) -> u64 {
        match (self.mode, self.kind) {
            (ActionGapMode::Declared, _) | (_, ActionGapKind::Performance) => 0,
            (ActionGapMode::Settled, ActionGapKind::Fixed) => self.fixed_ms,
            (ActionGapMode::Settled, ActionGapKind::Random) => {
                let span = self.random_max_ms.saturating_sub(self.random_min_ms) + 1;
                self.random_min_ms + seed % span
            }
        }
    }

    /// Whether a step at the pacing stage `stage` reads frames after its input.
    pub fn settled_at(&self, stage: PerformancePacingStage) -> bool {
        matches!(self.mode, ActionGapMode::Settled) && stage <= self.settled_max_stage
    }
}

/// The value `key` holds, of a key whose scalar is an integer or milliseconds, within `range`.
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

/// The word `key` holds, as `parse` reads it.
fn word<T>(
    value: &dyn Fn(&str) -> Option<FactScalar>,
    key: &str,
    parse: fn(&str) -> Option<T>,
) -> Result<T, ConfigFailure> {
    match value(key) {
        Some(FactScalar::String(token)) => parse(&token)
            .ok_or_else(|| ConfigFailure::invalid_value(key, ConfigValueProblem::OutOfRange)),
        _ => Err(ConfigFailure::invalid_value(
            key,
            ConfigValueProblem::WrongType,
        )),
    }
}

fn parse_mode(token: &str) -> Option<ActionGapMode> {
    match token {
        "declared" => Some(ActionGapMode::Declared),
        "settled" => Some(ActionGapMode::Settled),
        _ => None,
    }
}

fn parse_kind(token: &str) -> Option<ActionGapKind> {
    match token {
        "fixed" => Some(ActionGapKind::Fixed),
        "random" => Some(ActionGapKind::Random),
        "performance" => Some(ActionGapKind::Performance),
        _ => None,
    }
}

fn parse_stage(token: &str) -> Option<PerformancePacingStage> {
    match token {
        "s0" => Some(PerformancePacingStage::S0),
        "s1" => Some(PerformancePacingStage::S1),
        "s2" => Some(PerformancePacingStage::S2),
        "s3" => Some(PerformancePacingStage::S3),
        "s4" => Some(PerformancePacingStage::S4),
        _ => None,
    }
}

/// The keys as `key_of` spells each, each of its type and within its range, and `random_min_ms` at
/// most `random_max_ms` (`section_check` on the first); `config_value_invalid` for the first key
/// that is not.
fn parse_keys(
    value: &dyn Fn(&str) -> Option<FactScalar>,
    key_of: &dyn Fn(&str) -> String,
) -> Result<ActionGapConfig, ConfigFailure> {
    let random_min_key = key_of(RANDOM_MIN_MS);
    let random_min_ms = bounded(value, &random_min_key, GAP_RANGE)?;
    let random_max_ms = bounded(value, &key_of(RANDOM_MAX_MS), GAP_RANGE)?;
    if random_min_ms > random_max_ms {
        return Err(ConfigFailure::invalid_value(
            random_min_key,
            ConfigValueProblem::SectionCheck,
        ));
    }
    let settle_frames_key = key_of(SETTLE_FRAMES);
    let settle_change_key = key_of(SETTLE_CHANGE_MAX_MILLI);
    let settle_frames = bounded(value, &settle_frames_key, SETTLE_FRAMES_RANGE)?;
    let settle_change_max_milli = bounded(value, &settle_change_key, SETTLE_CHANGE_RANGE)?;
    Ok(ActionGapConfig {
        mode: word(value, &key_of(MODE), parse_mode)?,
        kind: word(value, &key_of(KIND), parse_kind)?,
        fixed_ms: bounded(value, &key_of(FIXED_MS), GAP_RANGE)?,
        random_min_ms,
        random_max_ms,
        poll_ms: bounded(value, &key_of(POLL_MS), POLL_RANGE)?,
        settle_frames: u32::try_from(settle_frames).map_err(|_| {
            ConfigFailure::invalid_value(settle_frames_key, ConfigValueProblem::OutOfRange)
        })?,
        settle_change_max_milli: u32::try_from(settle_change_max_milli).map_err(|_| {
            ConfigFailure::invalid_value(settle_change_key, ConfigValueProblem::OutOfRange)
        })?,
        settled_max_stage: word(value, &key_of(SETTLED_MAX_STAGE), parse_stage)?,
        fallback_hold_ms: bounded(value, &key_of(FALLBACK_HOLD_MS), FALLBACK_HOLD_RANGE)?,
    })
}

/// The section-level keys in effect, checked whole.
pub(crate) fn section(
    value: &dyn Fn(&str) -> Option<FactScalar>,
) -> Result<ActionGapConfig, ConfigFailure> {
    parse_keys(value, &|member: &str| member.to_owned())
}

/// The keys in effect for the instance whose registry id is `instance_id`, checked whole: each its
/// own when the configuration carries it, otherwise the section's.
pub(crate) fn instance(
    value: &dyn Fn(&str) -> Option<FactScalar>,
    instance_id: &str,
) -> Result<ActionGapConfig, ConfigFailure> {
    parse_keys(value, &|member: &str| {
        let own = instance_key(instance_id, member);
        if value(&own).is_some() {
            own
        } else {
            member.to_owned()
        }
    })
}
