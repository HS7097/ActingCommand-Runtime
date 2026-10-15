// SPDX-License-Identifier: AGPL-3.0-only

//! `performance.monitor.*`, `performance.load.*`, `performance.pressure.*` and the two pressure
//! streaks (Workflow #385 P-c, PERF §10.1): the monitor's values that were constants until now.
//! The streaks were already in the file; they keep their members `pressure_start_samples` and
//! `pressure_end_samples` of the section (M387 §5.1: a section moves with its file names
//! unchanged) and the start-up assembly reads them as before. The other keys are read by P-m.

use super::{
    AT_START, BASIS_POINTS, Values, at_least, between, exact_object, failure, integer_within,
    pair_within, row, rule,
};
use crate::codes::ConfigValueProblem;
use crate::effective::ConfigFailure;
use crate::table::{KeyRange, KeyRow, ValueType};

/// The sampling period, 1 000..=5 000 ms (§10.1).
pub const SAMPLE_INTERVAL_MS: &str = "performance.monitor.sample_interval_ms";
pub const CONTEXT_WINDOW_MS: &str = "performance.monitor.context_window_ms";
pub const SUMMARY_INTERVAL_MS: &str = "performance.monitor.summary_interval_ms";
/// Consecutive samples above a pressure's start threshold before it is recorded (1..=30).
pub const PRESSURE_START_SAMPLES: &str = "performance.pressure_start_samples";
/// Consecutive samples below a pressure's end threshold before it is ended (1..=30).
pub const PRESSURE_END_SAMPLES: &str = "performance.pressure_end_samples";
/// The start and end thresholds of each pressure kind (an object).
pub const THRESHOLDS: &str = "performance.pressure.thresholds";
/// What makes a pressure elevated or critical (an object).
pub const SEVERITY: &str = "performance.pressure.severity";

/// The most consecutive samples a pressure streak may ask for.
pub const MAX_PRESSURE_STREAK_SAMPLES: i64 = 30;

/// How the GPU's engines make one utilisation: the busiest engine type, or the sum.
const GPU_AGGREGATES: &[&str] = &["max_engine_type", "sum"];
/// `performance.pressure.gpu_aggregate`.
pub const GPU_AGGREGATE: &str = "performance.pressure.gpu_aggregate";

/// Workflow #385 P-m (PERF §3.4): how the GPU's engine utilisations make one value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuAggregate {
    /// Sum the engines of each engine type, then take the busiest type, as the task manager
    /// does (the default).
    MaxEngineType,
    /// Sum every engine, capped at 100 % (the measurement before P-m).
    Sum,
}

/// Workflow #385 P-m: `performance.pressure.gpu_aggregate` in effect.
pub(super) fn gpu_aggregate(values: &Values<'_>) -> Result<GpuAggregate, ConfigFailure> {
    match values.word(GPU_AGGREGATE)?.as_str() {
        "max_engine_type" => Ok(GpuAggregate::MaxEngineType),
        "sum" => Ok(GpuAggregate::Sum),
        _ => Err(failure(GPU_AGGREGATE, ConfigValueProblem::OutOfRange)),
    }
}

const READER: &str = "P-m";

pub(crate) static ROWS: &[KeyRow] = &[
    row(
        SAMPLE_INTERVAL_MS,
        ValueType::DurationMs,
        between(1_000, 5_000),
        AT_START,
        READER,
    ),
    row(
        CONTEXT_WINDOW_MS,
        ValueType::DurationMs,
        at_least(1),
        AT_START,
        READER,
    ),
    row(
        SUMMARY_INTERVAL_MS,
        ValueType::DurationMs,
        at_least(1),
        AT_START,
        READER,
    ),
    row(
        "performance.monitor.max_pipeline_samples",
        ValueType::Integer,
        at_least(1),
        AT_START,
        READER,
    ),
    row(
        "performance.monitor.process_coverage_min_bp",
        ValueType::Integer,
        BASIS_POINTS,
        AT_START,
        READER,
    ),
    row(
        "performance.monitor.max_system_samples",
        ValueType::Integer,
        at_least(1),
        AT_START,
        READER,
    ),
    row(
        "performance.monitor.max_consecutive_failures",
        ValueType::Integer,
        between(1, u16::MAX as i64),
        AT_START,
        READER,
    ),
    row(
        "performance.monitor.control_freshness_multiplier",
        ValueType::Integer,
        at_least(1),
        AT_START,
        READER,
    ),
    row(
        "performance.monitor.stutter_frame_gap_ms",
        ValueType::DurationMs,
        at_least(1),
        AT_START,
        READER,
    ),
    row(
        "performance.load.heavy_threshold_milli",
        ValueType::Integer,
        at_least(1),
        AT_START,
        READER,
    ),
    row(
        PRESSURE_START_SAMPLES,
        ValueType::Integer,
        between(1, MAX_PRESSURE_STREAK_SAMPLES),
        AT_START,
        "actingd",
    ),
    row(
        PRESSURE_END_SAMPLES,
        ValueType::Integer,
        between(1, MAX_PRESSURE_STREAK_SAMPLES),
        AT_START,
        "actingd",
    ),
    row(
        THRESHOLDS,
        ValueType::Object,
        KeyRange::Any,
        AT_START,
        READER,
    ),
    row(SEVERITY, ValueType::Object, KeyRange::Any, AT_START, READER),
    row(
        GPU_AGGREGATE,
        ValueType::String,
        KeyRange::Tokens(GPU_AGGREGATES),
        AT_START,
        READER,
    ),
];

/// The utilisation kinds, each `[start_bp, end_bp]`.
const UTILISATION_KINDS: [&str; 3] = ["cpu", "ram", "gpu"];
/// The disk kinds, each `[start, end]` above 0.
const DISK_KINDS: [&str; 2] = ["disk_queue_milli", "disk_latency_us"];
const PROCESS_CPU_BP: &str = "process_cpu_bp";
const PROCESS_IO_BPS: &str = "process_io_bps";

const CRITICAL_BP: &str = "critical_bp";
const ELEVATED_BP: &str = "elevated_bp";
const THIRD_PARTY_CRITICAL: &str = "third_party_critical";
const THIRD_PARTY_ELEVATED: &str = "third_party_elevated";

/// §10.1: each window and summary at least one sampling period; each pressure ends below where
/// it starts; an elevated pressure is never above a critical one.
pub(super) fn check(values: &Values<'_>) -> Result<(), ConfigFailure> {
    let sample_interval = values.integer(SAMPLE_INTERVAL_MS)?;
    for key in [CONTEXT_WINDOW_MS, SUMMARY_INTERVAL_MS] {
        rule(key, values.integer(key)? >= sample_interval)?;
    }
    check_thresholds(&values.json(THRESHOLDS)?)?;
    check_severity(&values.json(SEVERITY)?)
}

fn check_thresholds(thresholds: &serde_json::Value) -> Result<(), ConfigFailure> {
    let names = UTILISATION_KINDS
        .iter()
        .chain(DISK_KINDS.iter())
        .copied()
        .chain([PROCESS_CPU_BP, PROCESS_IO_BPS])
        .collect::<Vec<_>>();
    let members = exact_object(THRESHOLDS, thresholds, &names)?;
    for kind in UTILISATION_KINDS {
        let (start, end) = pair_within(THRESHOLDS, members.get(kind), 0, 10_000)?;
        rule(THRESHOLDS, end < start)?;
    }
    for kind in DISK_KINDS {
        let (start, end) = pair_within(THRESHOLDS, members.get(kind), 0, i64::MAX)?;
        rule(THRESHOLDS, start > 0 && end < start)?;
    }
    integer_within(THRESHOLDS, members.get(PROCESS_CPU_BP), 0, 10_000)?;
    integer_within(THRESHOLDS, members.get(PROCESS_IO_BPS), 1, i64::MAX)?;
    Ok(())
}

fn check_severity(severity: &serde_json::Value) -> Result<(), ConfigFailure> {
    let members = exact_object(
        SEVERITY,
        severity,
        &[
            CRITICAL_BP,
            ELEVATED_BP,
            THIRD_PARTY_CRITICAL,
            THIRD_PARTY_ELEVATED,
        ],
    )?;
    let critical = integer_within(SEVERITY, members.get(CRITICAL_BP), 0, 10_000)?;
    let elevated = integer_within(SEVERITY, members.get(ELEVATED_BP), 0, 10_000)?;
    rule(SEVERITY, elevated <= critical)?;
    // `[process count, peak process CPU in basis points]`.
    let (critical_count, critical_peak) =
        pair_within(SEVERITY, members.get(THIRD_PARTY_CRITICAL), 0, 10_000)?;
    let (elevated_count, elevated_peak) =
        pair_within(SEVERITY, members.get(THIRD_PARTY_ELEVATED), 0, 10_000)?;
    if critical_count == 0 || elevated_count == 0 {
        return Err(failure(SEVERITY, ConfigValueProblem::OutOfRange));
    }
    rule(
        SEVERITY,
        elevated_count <= critical_count && elevated_peak <= critical_peak,
    )
}
