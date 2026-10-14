// SPDX-License-Identifier: AGPL-3.0-only

//! `performance.pacing.*` (Workflow #385 P-c, PERF §10.3): the mode, the graded curve and what
//! each stage does, the stage-one pressure floor, the dispatch limits and clock jump the
//! performance controller reads today, the hold and the pause probe. `mode` is `observe` in
//! every release (§4.6); `pace` and `full` are a person's configuration, never a default.
//! The start-up assembly hands the heavy-dispatch limits and the clock jump to the host (P-c);
//! the other keys are read by P-m, P-k, P-s and P-h.

use super::{
    AT_START, STAGE_WORDS, Values, WHEN_USED, at_least, between, exact_object, failure,
    integer_within, row, rule, stage_index,
};
use crate::codes::ConfigValueProblem;
use crate::effective::ConfigFailure;
use crate::table::{KeyRange, KeyRow, ValueType};
use serde_json::Value;

/// `observe` (measure and record only), `pace` (stretch, never pause) or `full`.
pub const MODE: &str = "performance.pacing.mode";
/// `linear` or `step` (§4.1).
pub const INTERPOLATE: &str = "performance.pacing.interpolate";
/// The curve's points from `r_bp` 10 000 down to 0 (§4.1, a list).
pub const CURVE: &str = "performance.pacing.curve";
/// The highest factor a curve point may give.
pub const MAX_FACTOR_MILLI: &str = "performance.pacing.max_factor_milli";
/// The pressure kinds that hold a host at stage one at least (a list, §3.4).
pub const PRESSURE_FLOOR_KINDS: &str = "performance.pacing.pressure_floor.kinds";
/// How many heavy runs may be dispatched at once normally, and under pressure.
pub const HEAVY_DISPATCH_LIMIT_NORMAL: &str = "performance.pacing.heavy_dispatch_limit.normal";
pub const HEAVY_DISPATCH_LIMIT_PAUSED: &str = "performance.pacing.heavy_dispatch_limit.paused";
/// The jump between two controller inputs read as a clock jump.
pub const CLOCK_JUMP_MS: &str = "performance.pacing.clock_jump_ms";

/// The release default of `mode` in every release until Alice approves another (§4.6, §10.4).
pub const OBSERVE: &str = "observe";
const MODES: &[&str] = &[OBSERVE, "pace", "full"];
const INTERPOLATIONS: &[&str] = &["linear", "step"];
/// The pressure severities, lowest first.
const SEVERITIES: &[&str] = &["elevated", "high", "critical"];
/// The pressure kinds the floor may name (§3.4; `disk_capacity` may be added, default out).
const PRESSURE_KINDS: &[&str] = &[
    "cpu",
    "ram",
    "gpu",
    "disk_io",
    "disk_capacity",
    "third_party",
    "fullscreen_contention",
    "ledger_backlog",
];

/// The most points a curve may have (coordinator ruling, 2026-10-14): the largest count whose
/// `config.parameters` text, the curve's canonical JSON, stays within 1 024 bytes for every curve
/// the other rules allow. The longest such text puts `extra_retries` and `factor_milli` at
/// `i64::MAX` (19 digits, with `max_factor_milli` there too), every share at 1 000, `r_bp` at
/// 10 000 first, four digits in between and 0 last: 117 × points − 1 bytes, so 8 points take 935
/// bytes and 9 take 1 052.
pub const MAX_CURVE_POINTS: usize = 8;

/// The first and last `r_bp` of a curve.
const CURVE_TOP_BP: i64 = 10_000;
const CURVE_BOTTOM_BP: i64 = 0;
/// A factor of one.
const UNIT_FACTOR_MILLI: i64 = 1_000;
/// A full dispatch share.
const FULL_SHARE_MILLI: i64 = 1_000;
/// The position of the last stage a curve point may name, S3: S4 is computed, never a label
/// (§4.1).
const LAST_CURVE_STAGE_AT: usize = 3;

const R_BP: &str = "r_bp";
const POINT_STAGE: &str = "stage";
const FACTOR_MILLI: &str = "factor_milli";
const EXTRA_RETRIES: &str = "extra_retries";
const SHARE_MILLI: &str = "share_milli";

const fn at_start(
    name: &'static str,
    value_type: ValueType,
    range: KeyRange,
    reader: &'static str,
) -> KeyRow {
    row(name, value_type, range, AT_START, reader)
}

const fn when_used(
    name: &'static str,
    value_type: ValueType,
    range: KeyRange,
    reader: &'static str,
) -> KeyRow {
    row(name, value_type, range, WHEN_USED, reader)
}

pub(crate) static ROWS: &[KeyRow] = &[
    at_start(MODE, ValueType::String, KeyRange::Tokens(MODES), "P-m"),
    at_start(
        INTERPOLATE,
        ValueType::String,
        KeyRange::Tokens(INTERPOLATIONS),
        "P-m",
    ),
    when_used(CURVE, ValueType::List, KeyRange::Any, "P-m"),
    at_start(
        MAX_FACTOR_MILLI,
        ValueType::Integer,
        at_least(UNIT_FACTOR_MILLI),
        "P-m",
    ),
    at_start(
        "performance.pacing.hysteresis_bp",
        ValueType::Integer,
        between(0, 10_000),
        "P-m",
    ),
    at_start(
        "performance.pacing.degrade_sustain_ms",
        ValueType::DurationMs,
        at_least(0),
        "P-m",
    ),
    at_start(
        "performance.pacing.recover_sustain_ms",
        ValueType::DurationMs,
        at_least(0),
        "P-m",
    ),
    at_start(
        "performance.pacing.pause_sustain_ms",
        ValueType::DurationMs,
        at_least(0),
        "P-s",
    ),
    at_start(
        "performance.pacing.resume_sustain_ms",
        ValueType::DurationMs,
        at_least(0),
        "P-s",
    ),
    at_start(
        "performance.pacing.pause_cooldown_ms",
        ValueType::DurationMs,
        at_least(0),
        "P-s",
    ),
    at_start(
        "performance.pacing.pre_input_wait_base_ms",
        ValueType::DurationMs,
        at_least(0),
        "P-k",
    ),
    at_start(
        "performance.pacing.inter_run_gap_base_ms",
        ValueType::DurationMs,
        at_least(0),
        "P-s",
    ),
    at_start(PRESSURE_FLOOR_KINDS, ValueType::List, KeyRange::Any, "P-s"),
    at_start(
        "performance.pacing.pressure_floor.min_severity",
        ValueType::String,
        KeyRange::Tokens(SEVERITIES),
        "P-s",
    ),
    at_start(
        "performance.pacing.pressure_floor.factor_milli",
        ValueType::Integer,
        at_least(UNIT_FACTOR_MILLI),
        "P-s",
    ),
    at_start(
        "performance.pacing.pressure_floor.extra_retries",
        ValueType::Integer,
        at_least(0),
        "P-s",
    ),
    at_start(
        "performance.pacing.tax_exempt_from_stage",
        ValueType::String,
        STAGE_WORDS,
        "P-s",
    ),
    at_start(
        HEAVY_DISPATCH_LIMIT_NORMAL,
        ValueType::Integer,
        between(1, u16::MAX as i64),
        "P-c",
    ),
    at_start(
        HEAVY_DISPATCH_LIMIT_PAUSED,
        ValueType::Integer,
        between(1, u16::MAX as i64),
        "P-c",
    ),
    at_start(CLOCK_JUMP_MS, ValueType::DurationMs, at_least(1), "P-c"),
    when_used(
        "performance.pacing.hold.max_ms",
        ValueType::DurationMs,
        at_least(1),
        "P-h",
    ),
    when_used(
        "performance.pacing.hold.poll_ms",
        ValueType::DurationMs,
        at_least(1),
        "P-h",
    ),
    when_used(
        "performance.pacing.hold.probe_capture_interval_ms",
        ValueType::DurationMs,
        at_least(0),
        "P-h",
    ),
    when_used(
        "performance.pacing.hold.on_partial_pause",
        ValueType::Boolean,
        KeyRange::Any,
        "P-h",
    ),
    at_start(
        "performance.pacing.pause_probe.interval_ms",
        ValueType::DurationMs,
        at_least(0),
        "P-s",
    ),
    at_start(
        "performance.pacing.pause_probe.frames",
        ValueType::Integer,
        at_least(1),
        "P-s",
    ),
];

/// §4.1, §3.4: the curve's rules; the floor names known pressure kinds, each once; the limit
/// under pressure is not above the normal one.
pub(super) fn check(values: &Values<'_>) -> Result<(), ConfigFailure> {
    check_curve(&values.json(CURVE)?, values.integer(MAX_FACTOR_MILLI)?)?;
    check_pressure_kinds(&values.json(PRESSURE_FLOOR_KINDS)?)?;
    rule(
        HEAVY_DISPATCH_LIMIT_PAUSED,
        values.integer(HEAVY_DISPATCH_LIMIT_PAUSED)?
            <= values.integer(HEAVY_DISPATCH_LIMIT_NORMAL)?,
    )
}

/// One point of the curve, its stage as its position in the stage words.
struct CurvePoint {
    r_bp: i64,
    stage_at: usize,
    factor_milli: i64,
    share_milli: i64,
}

impl CurvePoint {
    fn read(point: &Value, max_factor_milli: i64) -> Result<Self, ConfigFailure> {
        let members = exact_object(
            CURVE,
            point,
            &[R_BP, POINT_STAGE, FACTOR_MILLI, EXTRA_RETRIES, SHARE_MILLI],
        )?;
        let word = members
            .get(POINT_STAGE)
            .and_then(Value::as_str)
            .ok_or_else(|| failure(CURVE, ConfigValueProblem::WrongType))?;
        let stage_at = stage_index(word)
            .filter(|at| *at <= LAST_CURVE_STAGE_AT)
            .ok_or_else(|| failure(CURVE, ConfigValueProblem::OutOfRange))?;
        integer_within(CURVE, members.get(EXTRA_RETRIES), 0, i64::MAX)?;
        Ok(Self {
            r_bp: integer_within(CURVE, members.get(R_BP), CURVE_BOTTOM_BP, CURVE_TOP_BP)?,
            stage_at,
            factor_milli: integer_within(
                CURVE,
                members.get(FACTOR_MILLI),
                UNIT_FACTOR_MILLI,
                max_factor_milli,
            )?,
            share_milli: integer_within(CURVE, members.get(SHARE_MILLI), 0, FULL_SHARE_MILLI)?,
        })
    }
}

/// §4.1: from `r_bp` 10 000 to 0, strictly falling; the factor never falls and the share never
/// rises as R falls; the stages S0 to S3 never step back; no two neighbours drop the share from
/// a full share straight to none ("不能一开始就直接停止"). At most [`MAX_CURVE_POINTS`] points.
fn check_curve(curve: &Value, max_factor_milli: i64) -> Result<(), ConfigFailure> {
    let points = curve
        .as_array()
        .ok_or_else(|| failure(CURVE, ConfigValueProblem::WrongType))?;
    if points.len() > MAX_CURVE_POINTS {
        return Err(failure(CURVE, ConfigValueProblem::OutOfRange));
    }
    let points = points
        .iter()
        .map(|point| CurvePoint::read(point, max_factor_milli))
        .collect::<Result<Vec<_>, _>>()?;
    let (Some(first), Some(last)) = (points.first(), points.last()) else {
        return Err(failure(CURVE, ConfigValueProblem::SectionCheck));
    };
    rule(
        CURVE,
        points.len() >= 2 && first.r_bp == CURVE_TOP_BP && last.r_bp == CURVE_BOTTOM_BP,
    )?;
    for pair in points.windows(2) {
        let [higher, lower] = pair else {
            continue;
        };
        rule(
            CURVE,
            lower.r_bp < higher.r_bp
                && lower.factor_milli >= higher.factor_milli
                && lower.share_milli <= higher.share_milli
                && lower.stage_at >= higher.stage_at
                && (higher.share_milli != FULL_SHARE_MILLI || lower.share_milli != 0),
        )?;
    }
    Ok(())
}

/// The floor's kinds: known words, each once.
fn check_pressure_kinds(kinds: &Value) -> Result<(), ConfigFailure> {
    let kinds = kinds
        .as_array()
        .ok_or_else(|| failure(PRESSURE_FLOOR_KINDS, ConfigValueProblem::WrongType))?;
    let mut seen = Vec::with_capacity(kinds.len());
    for kind in kinds {
        let kind = kind
            .as_str()
            .ok_or_else(|| failure(PRESSURE_FLOOR_KINDS, ConfigValueProblem::WrongType))?;
        if !PRESSURE_KINDS.contains(&kind) {
            return Err(failure(
                PRESSURE_FLOOR_KINDS,
                ConfigValueProblem::OutOfRange,
            ));
        }
        rule(PRESSURE_FLOOR_KINDS, !seen.contains(&kind))?;
        seen.push(kind);
    }
    Ok(())
}
