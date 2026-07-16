// SPDX-License-Identifier: AGPL-3.0-only

//! Typed performance facts shared by Runtime monitoring and failure records.

use crate::{EventId, SanitizationError};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

const MAX_CONTEXT_EVENTS: usize = 128;
const MAX_CONTEXT_PRESSURES: usize = 16;
const MAX_CONTEXT_METRICS: usize = 32;
const MAX_PROCESS_NAME_BYTES: usize = 260;
const MAX_PROCESS_SUMMARIES: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PerformanceMetric {
    CpuTotal,
    CpuPerCore,
    Ram,
    DiskQueueDepth,
    DiskLatency,
    Gpu,
    ProcessCpu,
    ProcessRam,
    ProcessIo,
    ForegroundProcess,
    FrameGap,
    CaptureLatency,
    RecognitionLatency,
    ActionEffectLatency,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PerformancePressureKind {
    Cpu,
    Ram,
    DiskIo,
    Gpu,
    ThirdParty,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PerformancePressureSeverity {
    Elevated,
    High,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PerformanceMonitorHealth {
    Healthy,
    Partial,
    Degraded,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PerformanceProcessOwnership {
    Runtime,
    Owned,
    ThirdParty,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PerformancePressureValue {
    Utilization {
        basis_points: u16,
    },
    DiskIo {
        queue_depth_milli: u32,
        latency_micros: u64,
    },
    ProcessLoad {
        process_count: u16,
        peak_cpu_basis_points: u16,
        peak_io_bytes_per_second: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerformancePressureRecord {
    pub kind: PerformancePressureKind,
    pub severity: PerformancePressureSeverity,
    pub started_at_unix_ms: u64,
    pub last_observed_at_unix_ms: u64,
    pub peak: PerformancePressureValue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerformanceProcessSummary {
    pub pid: u32,
    pub process_name: String,
    pub ownership: PerformanceProcessOwnership,
    pub cpu_basis_points: u16,
    pub working_set_bytes: u64,
    pub io_bytes_per_second: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerformanceForegroundSummary {
    pub process: PerformanceProcessSummary,
    pub fullscreen: bool,
}

/// A bounded, explicit view of performance evidence around an execution outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerformanceContext {
    pub window_start_unix_ms: u64,
    pub window_end_unix_ms: u64,
    pub health: PerformanceMonitorHealth,
    pub sample_count: u32,
    pub unavailable_metrics: Vec<PerformanceMetric>,
    pub pressures: Vec<PerformancePressureRecord>,
    pub max_cpu_basis_points: Option<u16>,
    pub max_ram_basis_points: Option<u16>,
    pub disk_queue_depth_p95_milli: Option<u32>,
    pub disk_latency_p95_micros: Option<u64>,
    pub max_gpu_basis_points: Option<u16>,
    pub max_frame_gap_ms: Option<u64>,
    pub max_capture_latency_ms: Option<u64>,
    pub max_recognition_latency_ms: Option<u64>,
    pub max_action_effect_latency_ms: Option<u64>,
    pub related_event_ids: Vec<EventId>,
}

impl PerformanceContext {
    pub fn unavailable(window_end_unix_ms: u64) -> Self {
        Self {
            window_start_unix_ms: window_end_unix_ms.saturating_sub(30_000).max(1),
            window_end_unix_ms,
            health: PerformanceMonitorHealth::Unavailable,
            sample_count: 0,
            unavailable_metrics: all_metrics(),
            pressures: Vec::new(),
            max_cpu_basis_points: None,
            max_ram_basis_points: None,
            disk_queue_depth_p95_milli: None,
            disk_latency_p95_micros: None,
            max_gpu_basis_points: None,
            max_frame_gap_ms: None,
            max_capture_latency_ms: None,
            max_recognition_latency_ms: None,
            max_action_effect_latency_ms: None,
            related_event_ids: Vec::new(),
        }
    }

    pub fn legacy_unavailable() -> Self {
        Self::unavailable(1)
    }

    pub fn pressure_observed(&self) -> bool {
        !self.pressures.is_empty()
    }

    pub fn validate(&self) -> Result<(), SanitizationError> {
        if self.window_start_unix_ms == 0
            || self.window_end_unix_ms < self.window_start_unix_ms
            || self.unavailable_metrics.len() > MAX_CONTEXT_METRICS
            || self.pressures.len() > MAX_CONTEXT_PRESSURES
            || self.related_event_ids.len() > MAX_CONTEXT_EVENTS
            || self
                .max_cpu_basis_points
                .is_some_and(|value| value > 10_000)
            || self
                .max_ram_basis_points
                .is_some_and(|value| value > 10_000)
            || self
                .max_gpu_basis_points
                .is_some_and(|value| value > 10_000)
        {
            return Err(SanitizationError::new(
                "invalid_performance_context",
                "perf_context",
            ));
        }
        let unique_metrics = self
            .unavailable_metrics
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            .len();
        let unique_pressures = self
            .pressures
            .iter()
            .map(|pressure| pressure.kind)
            .collect::<BTreeSet<_>>()
            .len();
        let unique_events = self
            .related_event_ids
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            .len();
        if (self.sample_count == 0) != (self.health == PerformanceMonitorHealth::Unavailable)
            || (self.health == PerformanceMonitorHealth::Healthy
                && !self.unavailable_metrics.is_empty())
            || (matches!(
                self.health,
                PerformanceMonitorHealth::Partial | PerformanceMonitorHealth::Unavailable
            ) && self.unavailable_metrics.is_empty())
            || unique_metrics != self.unavailable_metrics.len()
            || unique_pressures != self.pressures.len()
            || unique_events != self.related_event_ids.len()
        {
            return Err(SanitizationError::new(
                "invalid_performance_context_health",
                "perf_context",
            ));
        }
        for pressure in &self.pressures {
            pressure.validate()?;
            if pressure.started_at_unix_ms > self.window_end_unix_ms
                || pressure.last_observed_at_unix_ms < self.window_start_unix_ms
                || pressure.last_observed_at_unix_ms > self.window_end_unix_ms
            {
                return Err(SanitizationError::new(
                    "invalid_performance_pressure_time",
                    "perf_context",
                ));
            }
        }
        Ok(())
    }
}

impl PerformancePressureRecord {
    pub fn validate(&self) -> Result<(), SanitizationError> {
        if self.started_at_unix_ms == 0 || self.last_observed_at_unix_ms < self.started_at_unix_ms {
            return Err(SanitizationError::new(
                "invalid_performance_pressure_time",
                "performance_pressure",
            ));
        }
        match (&self.kind, &self.peak) {
            (
                PerformancePressureKind::Cpu
                | PerformancePressureKind::Ram
                | PerformancePressureKind::Gpu,
                PerformancePressureValue::Utilization { basis_points },
            ) if *basis_points <= 10_000 => Ok(()),
            (
                PerformancePressureKind::DiskIo,
                PerformancePressureValue::DiskIo {
                    queue_depth_milli,
                    latency_micros,
                },
            ) if *queue_depth_milli > 0 || *latency_micros > 0 => Ok(()),
            (
                PerformancePressureKind::ThirdParty,
                PerformancePressureValue::ProcessLoad {
                    process_count,
                    peak_cpu_basis_points,
                    ..
                },
            ) if *process_count > 0 && *peak_cpu_basis_points <= 10_000 => Ok(()),
            _ => Err(SanitizationError::new(
                "invalid_performance_pressure_value",
                "performance_pressure",
            )),
        }
    }
}

impl PerformanceProcessSummary {
    pub fn validate(&self) -> Result<(), SanitizationError> {
        if self.pid == 0
            || self.process_name.is_empty()
            || self.process_name.len() > MAX_PROCESS_NAME_BYTES
            || self.process_name.chars().any(char::is_control)
            || self.cpu_basis_points > 10_000
        {
            return Err(SanitizationError::new(
                "invalid_performance_process",
                "process",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PerformancePressureEventData {
    pub observed_at_unix_ms: u64,
    pub pressure: PerformancePressureRecord,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PerformanceStutterEventData {
    pub instance_id: String,
    pub observed_at_unix_ms: u64,
    pub frame_gap_ms: u64,
    pub capture_latency_ms: Option<u64>,
    pub recognition_latency_ms: Option<u64>,
    pub action_effect_latency_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PerformanceSummaryEventData {
    pub context: PerformanceContext,
    pub foreground: Option<PerformanceForegroundSummary>,
    pub owned_processes: Vec<PerformanceProcessSummary>,
    pub third_party_high_load: Vec<PerformanceProcessSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PerformanceMonitorStateEventData {
    pub observed_at_unix_ms: u64,
    pub health: PerformanceMonitorHealth,
    pub failure_code: Option<String>,
    pub consecutive_failures: u16,
    pub terminal: bool,
    pub unavailable_metrics: Vec<PerformanceMetric>,
}

pub(crate) fn validate_performance_summary(
    data: &PerformanceSummaryEventData,
) -> Result<(), SanitizationError> {
    data.context.validate()?;
    if data.owned_processes.len() > MAX_PROCESS_SUMMARIES
        || data.third_party_high_load.len() > MAX_PROCESS_SUMMARIES
    {
        return Err(SanitizationError::new(
            "invalid_performance_process_count",
            "processes",
        ));
    }
    if let Some(foreground) = &data.foreground {
        foreground.process.validate()?;
    }
    for process in data
        .owned_processes
        .iter()
        .chain(data.third_party_high_load.iter())
    {
        process.validate()?;
    }
    Ok(())
}

pub(crate) fn validate_performance_stutter(
    data: &PerformanceStutterEventData,
) -> Result<(), SanitizationError> {
    if data.instance_id.is_empty()
        || data.instance_id.len() > 128
        || data.instance_id.chars().any(char::is_control)
        || data.observed_at_unix_ms == 0
        || data.frame_gap_ms == 0
    {
        return Err(SanitizationError::new(
            "invalid_performance_stutter",
            "stutter",
        ));
    }
    Ok(())
}

pub(crate) fn validate_performance_monitor_state(
    data: &PerformanceMonitorStateEventData,
) -> Result<(), SanitizationError> {
    if data.observed_at_unix_ms == 0
        || data.unavailable_metrics.len() > MAX_CONTEXT_METRICS
        || data
            .unavailable_metrics
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            .len()
            != data.unavailable_metrics.len()
        || data.failure_code.as_deref().is_some_and(|code| {
            code.is_empty()
                || code.len() > 128
                || !code
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        })
        || (data.health == PerformanceMonitorHealth::Healthy && data.failure_code.is_some())
        || (data.health == PerformanceMonitorHealth::Degraded && data.failure_code.is_none())
        || (data.health == PerformanceMonitorHealth::Degraded && data.consecutive_failures == 0)
        || (data.health != PerformanceMonitorHealth::Degraded
            && (data.consecutive_failures != 0 || data.terminal))
    {
        return Err(SanitizationError::new(
            "invalid_performance_monitor_state",
            "monitor_state",
        ));
    }
    Ok(())
}

fn all_metrics() -> Vec<PerformanceMetric> {
    vec![
        PerformanceMetric::CpuTotal,
        PerformanceMetric::CpuPerCore,
        PerformanceMetric::Ram,
        PerformanceMetric::DiskQueueDepth,
        PerformanceMetric::DiskLatency,
        PerformanceMetric::Gpu,
        PerformanceMetric::ProcessCpu,
        PerformanceMetric::ProcessRam,
        PerformanceMetric::ProcessIo,
        PerformanceMetric::ForegroundProcess,
        PerformanceMetric::FrameGap,
        PerformanceMetric::CaptureLatency,
        PerformanceMetric::RecognitionLatency,
        PerformanceMetric::ActionEffectLatency,
    ]
}
