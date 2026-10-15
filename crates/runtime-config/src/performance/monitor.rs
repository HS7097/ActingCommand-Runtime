// SPDX-License-Identifier: AGPL-3.0-only

//! `performance.monitor.*`, `performance.load.*`, `performance.pressure.*` and the two pressure
//! streaks (Workflow #385 P-c, PERF §10.1): the monitor's values that were constants until now.
//! The streaks were already in the file; they keep their members `pressure_start_samples` and
//! `pressure_end_samples` of the section (M387 §5.1: a section moves with its file names
//! unchanged) and the start-up assembly reads them as before. The other keys are read by P-m;
//! [`MonitorSettings`] is every key of the row as P-m reads it (Workflow #385 PM-CFG).

use super::{
    AT_START, BASIS_POINTS, Values, at_least, between, exact_object, failure, integer_within,
    pair_within, row, rule,
};
use crate::codes::ConfigValueProblem;
use crate::effective::ConfigFailure;
use crate::table::{KeyRange, KeyRow, ValueType};
use actingcommand_contract::FactScalar;

/// The sampling period, 1 000..=5 000 ms (§10.1).
pub const SAMPLE_INTERVAL_MS: &str = "performance.monitor.sample_interval_ms";
pub const CONTEXT_WINDOW_MS: &str = "performance.monitor.context_window_ms";
pub const SUMMARY_INTERVAL_MS: &str = "performance.monitor.summary_interval_ms";
/// The most pipeline samples (and pending pipeline starts of each kind) kept.
pub const MAX_PIPELINE_SAMPLES: &str = "performance.monitor.max_pipeline_samples";
/// The least share of readable processes, in basis points, for the process metrics to count.
pub const PROCESS_COVERAGE_MIN_BP: &str = "performance.monitor.process_coverage_min_bp";
/// The most host samples kept.
pub const MAX_SYSTEM_SAMPLES: &str = "performance.monitor.max_system_samples";
/// Failures in a row of the sampler or of the pipeline that stop it.
pub const MAX_CONSECUTIVE_FAILURES: &str = "performance.monitor.max_consecutive_failures";
/// How many sampling periods a sample stays fresh for the controller.
pub const CONTROL_FRESHNESS_MULTIPLIER: &str = "performance.monitor.control_freshness_multiplier";
/// The least frame gap that is a stutter.
pub const STUTTER_FRAME_GAP_MS: &str = "performance.monitor.stutter_frame_gap_ms";
/// The least summed `cpu_milli + gpu_milli + io_milli` of a heavy workload.
pub const HEAVY_THRESHOLD_MILLI: &str = "performance.load.heavy_threshold_milli";
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
        MAX_PIPELINE_SAMPLES,
        ValueType::Integer,
        at_least(1),
        AT_START,
        READER,
    ),
    row(
        PROCESS_COVERAGE_MIN_BP,
        ValueType::Integer,
        BASIS_POINTS,
        AT_START,
        READER,
    ),
    row(
        MAX_SYSTEM_SAMPLES,
        ValueType::Integer,
        at_least(1),
        AT_START,
        READER,
    ),
    row(
        MAX_CONSECUTIVE_FAILURES,
        ValueType::Integer,
        between(1, u16::MAX as i64),
        AT_START,
        READER,
    ),
    row(
        CONTROL_FRESHNESS_MULTIPLIER,
        ValueType::Integer,
        at_least(1),
        AT_START,
        READER,
    ),
    row(
        STUTTER_FRAME_GAP_MS,
        ValueType::DurationMs,
        at_least(1),
        AT_START,
        READER,
    ),
    row(
        HEAVY_THRESHOLD_MILLI,
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

const CPU: &str = "cpu";
const RAM: &str = "ram";
const GPU: &str = "gpu";
/// The utilisation kinds, each `[start_bp, end_bp]`.
const UTILISATION_KINDS: [&str; 3] = [CPU, RAM, GPU];
const DISK_QUEUE_MILLI: &str = "disk_queue_milli";
const DISK_LATENCY_US: &str = "disk_latency_us";
/// The disk kinds, each `[start, end]` above 0.
const DISK_KINDS: [&str; 2] = [DISK_QUEUE_MILLI, DISK_LATENCY_US];
const PROCESS_CPU_BP: &str = "process_cpu_bp";
const PROCESS_IO_BPS: &str = "process_io_bps";

const CRITICAL_BP: &str = "critical_bp";
const ELEVATED_BP: &str = "elevated_bp";
const THIRD_PARTY_CRITICAL: &str = "third_party_critical";
const THIRD_PARTY_ELEVATED: &str = "third_party_elevated";

/// Workflow #385 PM-CFG (PERF §10.1; register §5 "Read by P-m"): every key of the monitor's row
/// in effect, as the monitor, its sampler and the controller read them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonitorSettings {
    /// `performance.monitor.sample_interval_ms`.
    pub sample_interval_ms: u64,
    /// `performance.monitor.context_window_ms`: how far back a performance context reaches.
    pub context_window_ms: u64,
    /// `performance.monitor.summary_interval_ms`: the period of the host summary.
    pub summary_interval_ms: u64,
    /// `performance.monitor.max_pipeline_samples`.
    pub max_pipeline_samples: usize,
    /// `performance.monitor.process_coverage_min_bp`.
    pub process_coverage_min_bp: u16,
    /// `performance.monitor.max_system_samples`.
    pub max_system_samples: usize,
    /// `performance.monitor.max_consecutive_failures`.
    pub max_consecutive_failures: u16,
    /// `performance.monitor.control_freshness_multiplier`: the controller's freshness window is
    /// this many sampling periods.
    pub control_freshness_multiplier: u64,
    /// `performance.monitor.stutter_frame_gap_ms`.
    pub stutter_frame_gap_ms: u64,
    /// `performance.load.heavy_threshold_milli`: read by the controller, which hands it to the
    /// policy evaluator.
    pub heavy_threshold_milli: u64,
    /// `performance.pressure_start_samples`.
    pub pressure_start_samples: u16,
    /// `performance.pressure_end_samples`.
    pub pressure_end_samples: u16,
    /// `performance.pressure.thresholds`.
    pub thresholds: PressureThresholds,
    /// `performance.pressure.severity`.
    pub severity: PressureSeverity,
    /// `performance.pressure.gpu_aggregate`.
    pub gpu_aggregate: GpuAggregate,
}

/// `performance.pressure.thresholds`: where each pressure kind starts and ends, each pair
/// `(start, end)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PressureThresholds {
    /// `cpu`, in basis points.
    pub cpu_bp: (u16, u16),
    /// `ram`, in basis points.
    pub ram_bp: (u16, u16),
    /// `gpu`, in basis points.
    pub gpu_bp: (u16, u16),
    /// `disk_queue_milli`: the disk queue length, in thousandths.
    pub disk_queue_milli: (u64, u64),
    /// `disk_latency_us`: the disk's mean transfer time, in microseconds.
    pub disk_latency_us: (u64, u64),
    /// `process_cpu_bp`: the CPU from which another program's process is under high load.
    pub process_cpu_bp: u16,
    /// `process_io_bps`: the I/O rate from which another program's process is under high load.
    pub process_io_bps: u64,
}

/// `performance.pressure.severity`: from where a pressure is critical, and from where it is
/// above the lowest severity (the record's `high`; below it a pressure is `elevated`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PressureSeverity {
    /// `critical_bp`: a utilisation at or above it is critical.
    pub critical_bp: u16,
    /// `elevated_bp`: a utilisation at or above it, and below `critical_bp`, is high.
    pub elevated_bp: u16,
    /// `third_party_critical`: `(process count, peak process CPU in basis points)`; reaching
    /// either makes other programs' load critical.
    pub third_party_critical: (u16, u16),
    /// `third_party_elevated`: the same pair, from which that load is high.
    pub third_party_elevated: (u16, u16),
}

impl MonitorSettings {
    /// The row from `value` (the configuration in effect), checked by §10.1's rules first as the
    /// whole-section check does: a refusal is `perf_config_invalid` naming the key.
    pub fn parse(value: &dyn Fn(&str) -> Option<FactScalar>) -> Result<Self, ConfigFailure> {
        let values = Values { value };
        // One-off red proof: no rule check.
        Ok(Self {
            sample_interval_ms: values.unsigned(SAMPLE_INTERVAL_MS)?,
            context_window_ms: values.unsigned(CONTEXT_WINDOW_MS)?,
            summary_interval_ms: values.unsigned(SUMMARY_INTERVAL_MS)?,
            max_pipeline_samples: values.unsigned(MAX_PIPELINE_SAMPLES)?,
            process_coverage_min_bp: values.unsigned(PROCESS_COVERAGE_MIN_BP)?,
            max_system_samples: values.unsigned(MAX_SYSTEM_SAMPLES)?,
            max_consecutive_failures: values.unsigned(MAX_CONSECUTIVE_FAILURES)?,
            control_freshness_multiplier: values.unsigned(CONTROL_FRESHNESS_MULTIPLIER)?,
            stutter_frame_gap_ms: values.unsigned(STUTTER_FRAME_GAP_MS)?,
            heavy_threshold_milli: values.unsigned(HEAVY_THRESHOLD_MILLI)?,
            pressure_start_samples: values.unsigned(PRESSURE_START_SAMPLES)?,
            pressure_end_samples: values.unsigned(PRESSURE_END_SAMPLES)?,
            thresholds: thresholds(&values.json(THRESHOLDS)?)?,
            severity: severity(&values.json(SEVERITY)?)?,
            // One-off red proof: the default aggregate whatever the file says.
            gpu_aggregate: GpuAggregate::MaxEngineType,
        })
    }
}

/// §10.1: each window and summary at least one sampling period; each pressure ends below where
/// it starts; an elevated pressure is never above a critical one.
pub(super) fn check(values: &Values<'_>) -> Result<(), ConfigFailure> {
    let sample_interval = values.integer(SAMPLE_INTERVAL_MS)?;
    for key in [CONTEXT_WINDOW_MS, SUMMARY_INTERVAL_MS] {
        rule(key, values.integer(key)? >= sample_interval)?;
    }
    thresholds(&values.json(THRESHOLDS)?)?;
    severity(&values.json(SEVERITY)?)?;
    Ok(())
}

/// A checked member value in the type its reader holds it in.
fn narrow<T: TryFrom<i64>>(key: &str, number: i64) -> Result<T, ConfigFailure> {
    T::try_from(number).map_err(|_| failure(key, ConfigValueProblem::OutOfRange))
}

fn thresholds(thresholds: &serde_json::Value) -> Result<PressureThresholds, ConfigFailure> {
    let names = UTILISATION_KINDS
        .iter()
        .chain(DISK_KINDS.iter())
        .copied()
        .chain([PROCESS_CPU_BP, PROCESS_IO_BPS])
        .collect::<Vec<_>>();
    let members = exact_object(THRESHOLDS, thresholds, &names)?;
    let utilisation = |kind: &str| -> Result<(u16, u16), ConfigFailure> {
        let (start, end) = pair_within(THRESHOLDS, members.get(kind), 0, 10_000)?;
        rule(THRESHOLDS, end < start)?;
        Ok((narrow(THRESHOLDS, start)?, narrow(THRESHOLDS, end)?))
    };
    let disk = |kind: &str| -> Result<(u64, u64), ConfigFailure> {
        let (start, end) = pair_within(THRESHOLDS, members.get(kind), 0, i64::MAX)?;
        rule(THRESHOLDS, start > 0 && end < start)?;
        Ok((narrow(THRESHOLDS, start)?, narrow(THRESHOLDS, end)?))
    };
    Ok(PressureThresholds {
        cpu_bp: utilisation(CPU)?,
        ram_bp: utilisation(RAM)?,
        gpu_bp: utilisation(GPU)?,
        disk_queue_milli: disk(DISK_QUEUE_MILLI)?,
        disk_latency_us: disk(DISK_LATENCY_US)?,
        process_cpu_bp: narrow(
            THRESHOLDS,
            integer_within(THRESHOLDS, members.get(PROCESS_CPU_BP), 0, 10_000)?,
        )?,
        process_io_bps: narrow(
            THRESHOLDS,
            integer_within(THRESHOLDS, members.get(PROCESS_IO_BPS), 1, i64::MAX)?,
        )?,
    })
}

fn severity(severity: &serde_json::Value) -> Result<PressureSeverity, ConfigFailure> {
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
    )?;
    Ok(PressureSeverity {
        critical_bp: narrow(SEVERITY, critical)?,
        elevated_bp: narrow(SEVERITY, elevated)?,
        third_party_critical: (
            narrow(SEVERITY, critical_count)?,
            narrow(SEVERITY, critical_peak)?,
        ),
        third_party_elevated: (
            narrow(SEVERITY, elevated_count)?,
            narrow(SEVERITY, elevated_peak)?,
        ),
    })
}
