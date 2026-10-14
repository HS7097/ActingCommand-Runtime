// SPDX-License-Identifier: AGPL-3.0-only

//! `performance.calibration.*` (Workflow #385 P-c, PERF §10.4, §4.5): the self-calibration of
//! which states go to which stage, its learning period, the trouble codes it counts, the
//! tolerances and the bounds of each stage's boundary. P-a reads them. The stage boundaries are
//! spelled `bounds.s1` to `bounds.s4`: a configuration key is lowercase (the outcome key
//! `config_key` is a token).

use super::{
    AT_START, BASIS_POINTS, STAGE_WORDS, Values, at_least, between, failure, pair_within, row,
    rule, stage_index,
};
use crate::codes::ConfigValueProblem;
use crate::effective::ConfigFailure;
use crate::recovery_ladder::{Listed, listed};
use crate::table::{KeyRange, KeyRow, ValueType};
use serde_json::Value;

/// The codes of a step that counts as trouble (a list, §4.5.1 item 1).
pub const TROUBLE_CODES: &str = "performance.calibration.trouble_codes";
/// The deepest stage of the learning period, S0 to S4 (S4: no cap).
pub const LEARNING_MAX_STAGE: &str = "performance.calibration.learning_max_stage";
/// The deepest stage a state with pressure only may reach, S0 to S2 (§4.5.3).
pub const PRESSURE_MAX_STAGE: &str = "performance.calibration.pressure_max_stage";
/// The trouble rates, per mille, from which a state goes to S1, S2, S3 and stops; strictly
/// rising.
pub const TOLERANCES: [&str; 4] = [
    "performance.calibration.tolerance.s0",
    "performance.calibration.tolerance.s1",
    "performance.calibration.tolerance.s2",
    "performance.calibration.tolerance.stop",
];
/// The `[min, max]` basis points each boundary of S1 to S4 may move within; deeper stages lie
/// lower.
pub const BOUNDS: [&str; 4] = [
    "performance.calibration.bounds.s1",
    "performance.calibration.bounds.s2",
    "performance.calibration.bounds.s3",
    "performance.calibration.bounds.s4",
];

const APPLY_MODES: &[&str] = &["auto", "propose", "off"];
/// The position of the deepest stage a pressure-only state may reach, S2 (§10.4: S3 and S4 are
/// refused).
const PRESSURE_MAX_STAGE_AT: usize = 2;

const READER: &str = "P-a";

const fn calibration(name: &'static str, value_type: ValueType, range: KeyRange) -> KeyRow {
    row(name, value_type, range, AT_START, READER)
}

pub(crate) static ROWS: &[KeyRow] = &[
    calibration(
        "performance.calibration.apply",
        ValueType::String,
        KeyRange::Tokens(APPLY_MODES),
    ),
    calibration(
        "performance.calibration.band_bp",
        ValueType::Integer,
        between(1, 10_000),
    ),
    calibration(TROUBLE_CODES, ValueType::List, KeyRange::Any),
    calibration(
        "performance.calibration.learn_min_days",
        ValueType::Integer,
        at_least(0),
    ),
    calibration(
        "performance.calibration.learn_min_active_ms",
        ValueType::DurationMs,
        at_least(0),
    ),
    calibration(LEARNING_MAX_STAGE, ValueType::String, STAGE_WORDS),
    calibration(
        "performance.calibration.learning_backstop_bp",
        ValueType::Integer,
        BASIS_POINTS,
    ),
    calibration(
        "performance.calibration.interval_ms",
        ValueType::DurationMs,
        at_least(1),
    ),
    calibration(
        "performance.calibration.min_state_steps",
        ValueType::Integer,
        at_least(1),
    ),
    calibration(TOLERANCES[0], ValueType::Integer, between(0, 1_000)),
    calibration(TOLERANCES[1], ValueType::Integer, between(0, 1_000)),
    calibration(TOLERANCES[2], ValueType::Integer, between(0, 1_000)),
    calibration(TOLERANCES[3], ValueType::Integer, between(0, 1_000)),
    calibration(BOUNDS[0], ValueType::List, KeyRange::Any),
    calibration(BOUNDS[1], ValueType::List, KeyRange::Any),
    calibration(BOUNDS[2], ValueType::List, KeyRange::Any),
    calibration(BOUNDS[3], ValueType::List, KeyRange::Any),
    calibration(
        "performance.calibration.max_shift_bp",
        ValueType::Integer,
        BASIS_POINTS,
    ),
    calibration(PRESSURE_MAX_STAGE, ValueType::String, STAGE_WORDS),
];

/// §10.4: the trouble codes are listed codes; `tolerance` rises strictly; each boundary's
/// `[min, max]` has `min < max` and lies no higher than the stage before it; a pressure-only
/// state reaches S2 at most.
pub(super) fn check(values: &Values<'_>) -> Result<(), ConfigFailure> {
    check_trouble_codes(&values.json(TROUBLE_CODES)?)?;
    let mut previous: Option<i64> = None;
    for key in TOLERANCES {
        let tolerance = values.integer(key)?;
        rule(key, previous.is_none_or(|lower| tolerance > lower))?;
        previous = Some(tolerance);
    }
    let mut previous: Option<(i64, i64)> = None;
    for key in BOUNDS {
        let bound = &values.json(key)?;
        let (min, max) = pair_within(key, Some(bound), 0, 10_000)?;
        rule(key, min < max)?;
        rule(
            key,
            previous.is_none_or(|(above_min, above_max)| min <= above_min && max <= above_max),
        )?;
        previous = Some((min, max));
    }
    let deepest = values.word(PRESSURE_MAX_STAGE)?;
    let deepest = stage_index(&deepest)
        .ok_or_else(|| failure(PRESSURE_MAX_STAGE, ConfigValueProblem::OutOfRange))?;
    rule(PRESSURE_MAX_STAGE, deepest <= PRESSURE_MAX_STAGE_AT)
}

/// Each trouble code is an active code of the embedded catalog.
fn check_trouble_codes(codes: &Value) -> Result<(), ConfigFailure> {
    let codes = codes
        .as_array()
        .ok_or_else(|| failure(TROUBLE_CODES, ConfigValueProblem::WrongType))?;
    for spelling in codes {
        let spelling = spelling
            .as_str()
            .ok_or_else(|| failure(TROUBLE_CODES, ConfigValueProblem::WrongType))?;
        match listed(spelling) {
            Listed::Active => {}
            Listed::Retired => {
                return Err(failure(TROUBLE_CODES, ConfigValueProblem::CodeRetired));
            }
            Listed::Unlisted => {
                return Err(failure(TROUBLE_CODES, ConfigValueProblem::CodeUnknown));
            }
        }
    }
    Ok(())
}
