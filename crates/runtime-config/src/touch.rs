// SPDX-License-Identifier: AGPL-3.0-only

//! The `touch` section (Workflow #406 T12; plan Q13; `contracts/v012-register.md` §1.136): how
//! the Runtime draws the point a box press lands on and how long it remembers where its presses
//! landed.
//!
//! - `sampling.sigma_ratio_milli`: the spread of a centre-hotspot draw, in thousandths of the
//!   box's width and of its height (250: the box's edge lies two spreads from its centre);
//! - `sampling.max_redraws`: how often a draw outside the box is drawn again before the box's
//!   centre is taken;
//! - `sampling.candidate_count`: how many candidates a single-region press draws, of which the
//!   landing history takes the one unused for the longest time (1: no choice);
//! - `history.points_per_box`: the most landing points one box remembers, the newest kept;
//! - `history.max_boxes`: the most boxes one instance remembers, the box unused longest
//!   forgotten (a memory bound).
//!
//! Host-wide; every key is read at each step of a run, so an instruction may change each while
//! the Runtime runs. Every default lives in the defaults file. A value outside its range is
//! `config_value_invalid` (`out_of_range`) at start, in `check-config` and in an instruction.

use crate::codes::ConfigValueProblem;
use crate::effective::ConfigFailure;
use crate::table::{DefaultSource, FilePath, KeyName, KeyRange, KeyRow, Settable, ValueType};
use actingcommand_contract::FactScalar;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The section's name: the first segment of its keys.
pub const SECTION: &str = "touch";
/// The spread of a centre-hotspot draw, in thousandths of the box's width and height.
pub const SIGMA_RATIO_MILLI: &str = "touch.sampling.sigma_ratio_milli";
/// How often a draw outside the box is drawn again before the centre is taken.
pub const MAX_REDRAWS: &str = "touch.sampling.max_redraws";
/// How many candidates a single-region press draws.
pub const CANDIDATE_COUNT: &str = "touch.sampling.candidate_count";
/// The most landing points one box remembers.
pub const POINTS_PER_BOX: &str = "touch.history.points_per_box";
/// The most boxes one instance remembers.
pub const MAX_BOXES: &str = "touch.history.max_boxes";

/// The bounds of each key (plan Q13). Schema, not thresholds.
const SIGMA_RATIO_RANGE: KeyRange = KeyRange::Integer { min: 1, max: 1_000 };
const MAX_REDRAWS_RANGE: KeyRange = KeyRange::Integer { min: 0, max: 64 };
const CANDIDATE_COUNT_RANGE: KeyRange = KeyRange::Integer { min: 1, max: 16 };
const POINTS_PER_BOX_RANGE: KeyRange = KeyRange::Integer { min: 1, max: 256 };
const MAX_BOXES_RANGE: KeyRange = KeyRange::Integer {
    min: 1,
    max: 65_536,
};

/// The section as the configuration file and the defaults file write it: every member may be
/// left out and none is unknown (M387 §5.3).
// Interface anchor `actingd-config` revision 7 (contracts/component-interfaces.md): a Runtime
// before Workflow #406 T12 refuses a file that names the section, because the file denies
// unknown members.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TouchFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sampling: Option<TouchSamplingFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    history: Option<TouchHistoryFile>,
}

/// The `touch.sampling` member.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TouchSamplingFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sigma_ratio_milli: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_redraws: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    candidate_count: Option<Value>,
}

/// The `touch.history` member.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TouchHistoryFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    points_per_box: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_boxes: Option<Value>,
}

const fn row(name: &'static str, range: KeyRange) -> KeyRow {
    KeyRow {
        name: KeyName::Exact(name),
        file_path: FilePath::Key,
        value_type: ValueType::Integer,
        range,
        default: DefaultSource::File,
        // Read at each step of a run, so an instruction takes effect at the next step.
        settable: Settable::Yes,
        reader: "406-T12",
        since: "0.12.5",
    }
}

/// One row per key of the section, in the order of plan Q13.
pub(crate) static ROWS: &[KeyRow] = &[
    row(SIGMA_RATIO_MILLI, SIGMA_RATIO_RANGE),
    row(MAX_REDRAWS, MAX_REDRAWS_RANGE),
    row(CANDIDATE_COUNT, CANDIDATE_COUNT_RANGE),
    row(POINTS_PER_BOX, POINTS_PER_BOX_RANGE),
    row(MAX_BOXES, MAX_BOXES_RANGE),
];

/// The `touch` section in effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TouchConfig {
    /// The spread of a centre-hotspot draw, in thousandths (`1..=1000`).
    pub sigma_ratio_milli: u32,
    /// How often a draw outside the box is drawn again (`0..=64`).
    pub max_redraws: u32,
    /// How many candidates a single-region press draws (`1..=16`).
    pub candidate_count: u32,
    /// The most landing points one box remembers (`1..=256`).
    pub points_per_box: u32,
    /// The most boxes one instance remembers (`1..=65536`).
    pub max_boxes: u32,
}

/// The integer `key` holds, within `range`; `config_value_invalid` for `key` otherwise.
fn bounded(
    value: &dyn Fn(&str) -> Option<FactScalar>,
    key: &str,
    range: KeyRange,
) -> Result<u32, ConfigFailure> {
    let invalid = |problem| ConfigFailure::invalid_value(key, problem).with_range(range);
    let Some(FactScalar::Integer(number)) = value(key) else {
        return Err(invalid(ConfigValueProblem::WrongType));
    };
    let within = match range {
        KeyRange::Integer { min, max } => (min..=max).contains(&number),
        KeyRange::Any | KeyRange::Tokens(_) => true,
    };
    if !within {
        return Err(invalid(ConfigValueProblem::OutOfRange));
    }
    u32::try_from(number).map_err(|_| invalid(ConfigValueProblem::OutOfRange))
}

impl TouchConfig {
    /// The section from `value` (the configuration in effect), each key an integer within its
    /// range; `config_value_invalid` for the first key that is not.
    pub fn parse(value: &dyn Fn(&str) -> Option<FactScalar>) -> Result<Self, ConfigFailure> {
        Ok(Self {
            sigma_ratio_milli: bounded(value, SIGMA_RATIO_MILLI, SIGMA_RATIO_RANGE)?,
            max_redraws: bounded(value, MAX_REDRAWS, MAX_REDRAWS_RANGE)?,
            candidate_count: bounded(value, CANDIDATE_COUNT, CANDIDATE_COUNT_RANGE)?,
            points_per_box: bounded(value, POINTS_PER_BOX, POINTS_PER_BOX_RANGE)?,
            max_boxes: bounded(value, MAX_BOXES, MAX_BOXES_RANGE)?,
        })
    }
}
