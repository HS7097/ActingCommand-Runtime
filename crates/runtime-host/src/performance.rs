// SPDX-License-Identifier: AGPL-3.0-only

//! Bounded Runtime performance monitoring and failure-context projection.

use crate::performance_control::PerformanceControlObservation;
use crate::{RuntimeHostError, RuntimeHostResult};
use actingcommand_contract::{
    ActionId, EventId, EventSeverity, EventType, FrameId, PerformanceContext,
    PerformanceControlEventData, PerformanceControlLevel, PerformanceDeadlineDisposition,
    PerformanceForegroundSummary, PerformanceMetric, PerformanceMonitorHealth,
    PerformanceMonitorStateEventData, PerformancePressureEventData, PerformancePressureKind,
    PerformancePressureRecord, PerformancePressureSeverity, PerformancePressureValue,
    PerformanceProcessOwnership, PerformanceProcessSummary, PerformanceStutterEventData,
    PerformanceSummaryEventData, RecognitionId, RuntimeErrorCode,
};
use actingcommand_host_metrics::{
    HostMetric, HostSample, HostSampler, ProcessLoadThresholds,
    ProcessOwnership as HostProcessOwnership, ProcessSample as HostProcessSample,
};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::Duration;

const BASIS_POINTS_MAX: u16 = 10_000;
const DEFAULT_SAMPLE_INTERVAL: Duration = Duration::from_secs(2);
const DEFAULT_CONTEXT_WINDOW: Duration = Duration::from_secs(30);
const DEFAULT_SUMMARY_INTERVAL: Duration = Duration::from_secs(60);
const DEFAULT_MAX_SYSTEM_SAMPLES: usize = 64;
const DEFAULT_MAX_PIPELINE_SAMPLES: usize = 4096;
const DEFAULT_MAX_EVENT_REFERENCES: usize = 512;
const DEFAULT_MAX_CONSECUTIVE_FAILURES: u16 = 3;
const DEFAULT_TOP_PROCESS_COUNT: usize = 5;
const MAX_OWNED_PROCESS_COUNT: usize = 32;

#[derive(Debug, Clone)]
pub struct PerformanceMonitorConfig {
    sample_interval: Duration,
    context_window: Duration,
    summary_interval: Duration,
    max_system_samples: usize,
    max_pipeline_samples: usize,
    max_event_references: usize,
    max_consecutive_failures: u16,
    top_process_count: usize,
    owned_processes: BTreeMap<u32, String>,
    thresholds: PerformanceThresholds,
}

impl Default for PerformanceMonitorConfig {
    fn default() -> Self {
        Self {
            sample_interval: DEFAULT_SAMPLE_INTERVAL,
            context_window: DEFAULT_CONTEXT_WINDOW,
            summary_interval: DEFAULT_SUMMARY_INTERVAL,
            max_system_samples: DEFAULT_MAX_SYSTEM_SAMPLES,
            max_pipeline_samples: DEFAULT_MAX_PIPELINE_SAMPLES,
            max_event_references: DEFAULT_MAX_EVENT_REFERENCES,
            max_consecutive_failures: DEFAULT_MAX_CONSECUTIVE_FAILURES,
            top_process_count: DEFAULT_TOP_PROCESS_COUNT,
            owned_processes: BTreeMap::new(),
            thresholds: PerformanceThresholds::default(),
        }
    }
}

impl PerformanceMonitorConfig {
    pub fn with_owned_process(mut self, pid: u32, label: impl Into<String>) -> Self {
        self.owned_processes.insert(pid, label.into());
        self
    }

    pub fn sample_interval(&self) -> Duration {
        self.sample_interval
    }

    pub fn validate(&self) -> RuntimeHostResult<()> {
        if !(Duration::from_secs(1)..=Duration::from_secs(5)).contains(&self.sample_interval)
            || self.context_window < self.sample_interval
            || self.summary_interval < self.sample_interval
            || self.max_system_samples == 0
            || self.max_pipeline_samples == 0
            || self.max_event_references == 0
            || self.max_consecutive_failures == 0
            || self.top_process_count == 0
            || self.top_process_count > 32
            || self.owned_processes.len() > MAX_OWNED_PROCESS_COUNT
            || self
                .owned_processes
                .iter()
                .any(|(pid, label)| !valid_process_label(*pid, label))
        {
            return Err(performance_fatal(
                "performance_config_invalid",
                "validate_performance_config",
            ));
        }
        self.thresholds.validate()
    }
}

#[derive(Debug, Clone)]
pub(crate) struct PerformanceThresholds {
    pub(crate) cpu_start_basis_points: u16,
    pub(crate) cpu_end_basis_points: u16,
    pub(crate) ram_start_basis_points: u16,
    pub(crate) ram_end_basis_points: u16,
    pub(crate) gpu_start_basis_points: u16,
    pub(crate) gpu_end_basis_points: u16,
    pub(crate) disk_queue_start_milli: u32,
    pub(crate) disk_queue_end_milli: u32,
    pub(crate) disk_latency_start_micros: u64,
    pub(crate) disk_latency_end_micros: u64,
    pub(crate) process_cpu_start_basis_points: u16,
    pub(crate) process_io_start_bytes_per_second: u64,
    pub(crate) stutter_frame_gap_ms: u64,
}

impl Default for PerformanceThresholds {
    fn default() -> Self {
        Self {
            cpu_start_basis_points: 8_500,
            cpu_end_basis_points: 7_000,
            ram_start_basis_points: 9_000,
            ram_end_basis_points: 8_000,
            gpu_start_basis_points: 9_000,
            gpu_end_basis_points: 7_500,
            disk_queue_start_milli: 4_000,
            disk_queue_end_milli: 2_000,
            disk_latency_start_micros: 50_000,
            disk_latency_end_micros: 25_000,
            process_cpu_start_basis_points: 4_000,
            process_io_start_bytes_per_second: 64 * 1024 * 1024,
            stutter_frame_gap_ms: 1_000,
        }
    }
}

impl PerformanceThresholds {
    fn validate(&self) -> RuntimeHostResult<()> {
        if self.cpu_start_basis_points > BASIS_POINTS_MAX
            || self.cpu_end_basis_points >= self.cpu_start_basis_points
            || self.ram_start_basis_points > BASIS_POINTS_MAX
            || self.ram_end_basis_points >= self.ram_start_basis_points
            || self.gpu_start_basis_points > BASIS_POINTS_MAX
            || self.gpu_end_basis_points >= self.gpu_start_basis_points
            || self.disk_queue_start_milli == 0
            || self.disk_queue_end_milli >= self.disk_queue_start_milli
            || self.disk_latency_start_micros == 0
            || self.disk_latency_end_micros >= self.disk_latency_start_micros
            || self.process_cpu_start_basis_points > BASIS_POINTS_MAX
            || self.process_io_start_bytes_per_second == 0
            || self.stutter_frame_gap_ms == 0
        {
            return Err(performance_fatal(
                "performance_threshold_invalid",
                "validate_performance_config",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipelinePerformanceSignal {
    instance_id: String,
    observed_at_unix_ms: u64,
    frame_gap_ms: Option<u64>,
    capture_latency_ms: Option<u64>,
    recognition_latency_ms: Option<u64>,
    action_effect_latency_ms: Option<u64>,
}

impl PipelinePerformanceSignal {
    pub fn new(
        instance_id: impl Into<String>,
        observed_at_unix_ms: u64,
        frame_gap_ms: u64,
    ) -> RuntimeHostResult<Self> {
        let signal = Self {
            instance_id: instance_id.into(),
            observed_at_unix_ms,
            frame_gap_ms: Some(frame_gap_ms),
            capture_latency_ms: None,
            recognition_latency_ms: None,
            action_effect_latency_ms: None,
        };
        signal.validate()?;
        Ok(signal)
    }

    pub fn with_capture_latency(mut self, latency_ms: u64) -> RuntimeHostResult<Self> {
        self.capture_latency_ms = Some(latency_ms);
        self.validate()?;
        Ok(self)
    }

    pub fn with_recognition_latency(mut self, latency_ms: u64) -> RuntimeHostResult<Self> {
        self.recognition_latency_ms = Some(latency_ms);
        self.validate()?;
        Ok(self)
    }

    pub fn with_action_effect_latency(mut self, latency_ms: u64) -> RuntimeHostResult<Self> {
        self.action_effect_latency_ms = Some(latency_ms);
        self.validate()?;
        Ok(self)
    }

    fn validate(&self) -> RuntimeHostResult<()> {
        if self.instance_id.is_empty()
            || self.instance_id.len() > 128
            || self.instance_id.chars().any(char::is_control)
            || self.observed_at_unix_ms == 0
            || self.frame_gap_ms.is_some_and(|value| value == 0)
            || (self.frame_gap_ms.is_none()
                && self.capture_latency_ms.is_none()
                && self.recognition_latency_ms.is_none()
                && self.action_effect_latency_ms.is_none())
        {
            return Err(performance_fatal(
                "performance_pipeline_signal_invalid",
                "record_performance_pipeline_signal",
            ));
        }
        Ok(())
    }

    fn observed(
        instance_id: String,
        observed_at_unix_ms: u64,
        frame_gap_ms: Option<u64>,
        capture_latency_ms: Option<u64>,
        recognition_latency_ms: Option<u64>,
        action_effect_latency_ms: Option<u64>,
    ) -> RuntimeHostResult<Self> {
        let signal = Self {
            instance_id,
            observed_at_unix_ms,
            frame_gap_ms,
            capture_latency_ms,
            recognition_latency_ms,
            action_effect_latency_ms,
        };
        signal.validate()?;
        Ok(signal)
    }
}

pub(crate) struct PipelineEventObservation {
    pub(crate) event_type: EventType,
    pub(crate) instance_id: String,
    pub(crate) observed_at_unix_ms: u64,
    pub(crate) frame_id: Option<FrameId>,
    pub(crate) recognition_id: Option<RecognitionId>,
    pub(crate) action_id: Option<ActionId>,
}

#[derive(Clone)]
struct PendingPipelineMeasurement {
    instance_id: String,
    started_at_unix_ms: u64,
    frame_id: Option<FrameId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RawProcessSample {
    pub(crate) pid: u32,
    pub(crate) process_name: String,
    pub(crate) ownership: PerformanceProcessOwnership,
    pub(crate) cpu_basis_points: u16,
    pub(crate) working_set_bytes: u64,
    pub(crate) io_bytes_per_second: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RawSystemSample {
    pub(crate) observed_at_unix_ms: u64,
    pub(crate) cpu_total_basis_points: u16,
    pub(crate) cpu_per_core_basis_points: Vec<u16>,
    pub(crate) ram_used_basis_points: u16,
    pub(crate) disk_queue_depth_milli: Option<u32>,
    pub(crate) disk_latency_micros: Option<u64>,
    pub(crate) gpu_basis_points: Option<u16>,
    pub(crate) foreground: Option<PerformanceForegroundSummary>,
    pub(crate) owned_processes: Vec<RawProcessSample>,
    pub(crate) third_party_high_load: Vec<RawProcessSample>,
    pub(crate) unavailable_metrics: Vec<PerformanceMetric>,
}

pub(crate) fn system_performance_sampler() -> Result<Box<dyn HostSampler>, &'static str> {
    actingcommand_host_metrics::system_sampler()
}

#[derive(Debug, Clone)]
pub(crate) enum PerformanceSemanticEvent {
    PressureStarted(PerformancePressureEventData),
    PressureEnded(PerformancePressureEventData),
    StutterDetected(PerformanceStutterEventData),
    Summary(Box<PerformanceSummaryEventData>),
    MonitorDegraded(PerformanceMonitorStateEventData),
    MonitorRecovered(PerformanceMonitorStateEventData),
    BalanceChanged(PerformanceControlEventData),
}

impl PerformanceSemanticEvent {
    pub(crate) fn severity(&self) -> EventSeverity {
        match self {
            Self::PressureStarted(data) => match data.pressure.severity {
                PerformancePressureSeverity::Elevated => EventSeverity::Warning,
                PerformancePressureSeverity::High => EventSeverity::Error,
                PerformancePressureSeverity::Critical => EventSeverity::Error,
            },
            Self::PressureEnded(_) | Self::Summary(_) | Self::MonitorRecovered(_) => {
                EventSeverity::Info
            }
            Self::MonitorDegraded(data) if data.terminal => EventSeverity::Error,
            Self::BalanceChanged(data)
                if data.deadline_disposition
                    == Some(PerformanceDeadlineDisposition::CapacityFailure)
                    || data.level.rank() >= PerformanceControlLevel::QosReduced.rank() =>
            {
                EventSeverity::Error
            }
            Self::BalanceChanged(data)
                if data.recovery || data.level == PerformanceControlLevel::Normal =>
            {
                EventSeverity::Info
            }
            Self::StutterDetected(_) | Self::MonitorDegraded(_) => EventSeverity::Warning,
            Self::BalanceChanged(_) => EventSeverity::Warning,
        }
    }

    pub(crate) fn observed_at_unix_ms(&self) -> u64 {
        match self {
            Self::PressureStarted(data) | Self::PressureEnded(data) => data.observed_at_unix_ms,
            Self::StutterDetected(data) => data.observed_at_unix_ms,
            Self::Summary(data) => data.context.window_end_unix_ms,
            Self::MonitorDegraded(data) | Self::MonitorRecovered(data) => data.observed_at_unix_ms,
            Self::BalanceChanged(data) => data.observed_at_unix_ms,
        }
    }
}

pub(crate) struct PerformanceTick {
    pub(crate) events: Vec<PerformanceSemanticEvent>,
    pub(crate) stop_sampling: bool,
}

#[derive(Clone, Copy)]
enum MonitorFailureDomain {
    Sampler,
    Pipeline,
}

#[derive(Clone)]
struct ActivePressure {
    record: PerformancePressureRecord,
    event_id: Option<EventId>,
}

#[derive(Clone)]
struct TimedEventReference {
    observed_at_unix_ms: u64,
    event_id: EventId,
}

pub(crate) struct PerformanceMonitor {
    config: Option<PerformanceMonitorConfig>,
    sampler: Option<Box<dyn HostSampler>>,
    sampler_start_error: Option<&'static str>,
    system_samples: VecDeque<RawSystemSample>,
    pipeline_samples: VecDeque<PipelinePerformanceSignal>,
    active_pressures: BTreeMap<PerformancePressureKind, ActivePressure>,
    event_references: VecDeque<TimedEventReference>,
    degraded_metrics: BTreeSet<PerformanceMetric>,
    capture_starts: BTreeMap<FrameId, PendingPipelineMeasurement>,
    recognition_starts: BTreeMap<RecognitionId, PendingPipelineMeasurement>,
    action_starts: BTreeMap<ActionId, PendingPipelineMeasurement>,
    last_capture_completed: BTreeMap<String, u64>,
    health: PerformanceMonitorHealth,
    consecutive_sampler_failures: u16,
    consecutive_pipeline_failures: u16,
    sampling_stopped: bool,
    pipeline_stopped: bool,
    last_summary_unix_ms: Option<u64>,
    last_control_sample_unix_ms: Option<u64>,
}

impl PerformanceMonitor {
    pub(crate) fn disabled() -> Self {
        Self {
            config: None,
            sampler: None,
            sampler_start_error: None,
            system_samples: VecDeque::new(),
            pipeline_samples: VecDeque::new(),
            active_pressures: BTreeMap::new(),
            event_references: VecDeque::new(),
            degraded_metrics: BTreeSet::new(),
            capture_starts: BTreeMap::new(),
            recognition_starts: BTreeMap::new(),
            action_starts: BTreeMap::new(),
            last_capture_completed: BTreeMap::new(),
            health: PerformanceMonitorHealth::Unavailable,
            consecutive_sampler_failures: 0,
            consecutive_pipeline_failures: 0,
            sampling_stopped: false,
            pipeline_stopped: false,
            last_summary_unix_ms: None,
            last_control_sample_unix_ms: None,
        }
    }

    pub(crate) fn enabled(
        mut config: PerformanceMonitorConfig,
        sampler: Result<Box<dyn HostSampler>, &'static str>,
    ) -> RuntimeHostResult<Self> {
        config
            .owned_processes
            .entry(std::process::id())
            .or_insert_with(|| "actingcommand-runtime".to_owned());
        config.validate()?;
        let (sampler, sampler_start_error) = match sampler {
            Ok(sampler) => (Some(sampler), None),
            Err(error) => (None, Some(error)),
        };
        Ok(Self {
            config: Some(config),
            sampler,
            sampler_start_error,
            system_samples: VecDeque::new(),
            pipeline_samples: VecDeque::new(),
            active_pressures: BTreeMap::new(),
            event_references: VecDeque::new(),
            degraded_metrics: BTreeSet::new(),
            capture_starts: BTreeMap::new(),
            recognition_starts: BTreeMap::new(),
            action_starts: BTreeMap::new(),
            last_capture_completed: BTreeMap::new(),
            health: PerformanceMonitorHealth::Unavailable,
            consecutive_sampler_failures: 0,
            consecutive_pipeline_failures: 0,
            sampling_stopped: false,
            pipeline_stopped: false,
            last_summary_unix_ms: None,
            last_control_sample_unix_ms: None,
        })
    }

    pub(crate) fn sample_interval(&self) -> Option<Duration> {
        self.config.as_ref().map(|config| config.sample_interval)
    }

    pub(crate) fn accepts_pipeline_events(&self) -> bool {
        self.config.is_some() && !self.pipeline_stopped
    }

    pub(crate) fn tick(&mut self, observed_at_unix_ms: u64) -> RuntimeHostResult<PerformanceTick> {
        if self.sampling_stopped {
            return Ok(PerformanceTick {
                events: Vec::new(),
                stop_sampling: true,
            });
        }
        let Some(config) = self.config.clone() else {
            return Err(performance_fatal(
                "performance_monitor_disabled",
                "sample_performance_monitor",
            ));
        };
        let sample = if let Some(error) = self.sampler_start_error.take() {
            Err(error)
        } else if let Some(sampler) = self.sampler.as_mut() {
            sampler.sample(
                observed_at_unix_ms,
                &config.owned_processes,
                config.top_process_count,
                ProcessLoadThresholds {
                    cpu_basis_points: config.thresholds.process_cpu_start_basis_points,
                    io_bytes_per_second: config.thresholds.process_io_start_bytes_per_second,
                },
            )
        } else {
            Err("performance_sampler_unavailable")
        };
        match sample {
            Ok(sample) => self.ingest_system_sample(raw_system_sample(sample)),
            Err(code) => self.ingest_monitor_failure(
                observed_at_unix_ms,
                code,
                host_metrics(),
                MonitorFailureDomain::Sampler,
            ),
        }
    }

    pub(crate) fn record_pipeline_signal(
        &mut self,
        signal: PipelinePerformanceSignal,
    ) -> RuntimeHostResult<Vec<PerformanceSemanticEvent>> {
        signal.validate()?;
        let Some(config) = self.config.as_ref() else {
            return Err(performance_fatal(
                "performance_monitor_disabled",
                "record_performance_pipeline_signal",
            ));
        };
        if self.pipeline_stopped {
            return Err(performance_fatal(
                "performance_monitor_unavailable",
                "record_performance_pipeline_signal",
            ));
        }
        if self
            .pipeline_samples
            .back()
            .is_some_and(|previous| previous.observed_at_unix_ms > signal.observed_at_unix_ms)
        {
            return Err(performance_fatal(
                "performance_pipeline_time_invalid",
                "record_performance_pipeline_signal",
            ));
        }
        let mut events = Vec::new();
        if signal
            .frame_gap_ms
            .is_some_and(|frame_gap_ms| frame_gap_ms >= config.thresholds.stutter_frame_gap_ms)
        {
            events.push(PerformanceSemanticEvent::StutterDetected(
                PerformanceStutterEventData {
                    instance_id: signal.instance_id.clone(),
                    observed_at_unix_ms: signal.observed_at_unix_ms,
                    frame_gap_ms: signal.frame_gap_ms.ok_or_else(|| {
                        performance_fatal(
                            "performance_frame_gap_missing",
                            "record_performance_pipeline_signal",
                        )
                    })?,
                    capture_latency_ms: signal.capture_latency_ms,
                    recognition_latency_ms: signal.recognition_latency_ms,
                    action_effect_latency_ms: signal.action_effect_latency_ms,
                },
            ));
        }
        self.pipeline_samples.push_back(signal);
        trim_queue(
            &mut self.pipeline_samples,
            config.max_pipeline_samples,
            |sample| sample.observed_at_unix_ms,
            config.context_window,
        )?;
        Ok(events)
    }

    pub(crate) fn observe_pipeline_event(
        &mut self,
        observation: PipelineEventObservation,
    ) -> RuntimeHostResult<Vec<PerformanceSemanticEvent>> {
        if self.config.is_none() || self.pipeline_stopped {
            return Ok(Vec::new());
        }
        validate_pipeline_observation(&observation)?;
        let maximum = self
            .config
            .as_ref()
            .ok_or_else(|| {
                performance_fatal(
                    "performance_monitor_disabled",
                    "observe_performance_pipeline_event",
                )
            })?
            .max_pipeline_samples;
        match observation.event_type {
            EventType::CaptureRequested => {
                let frame_id = required_frame_id(&observation)?;
                insert_pending(
                    &mut self.capture_starts,
                    frame_id,
                    PendingPipelineMeasurement {
                        instance_id: observation.instance_id,
                        started_at_unix_ms: observation.observed_at_unix_ms,
                        frame_id: Some(frame_id),
                    },
                    maximum,
                )?;
                Ok(Vec::new())
            }
            EventType::CaptureCompleted => {
                let frame_id = required_frame_id(&observation)?;
                let pending = take_pending(&mut self.capture_starts, &frame_id)?;
                validate_pending_instance(&pending, &observation.instance_id)?;
                let capture_latency_ms =
                    elapsed_ms(pending.started_at_unix_ms, observation.observed_at_unix_ms)?;
                let frame_gap_ms = self
                    .last_capture_completed
                    .insert(
                        observation.instance_id.clone(),
                        observation.observed_at_unix_ms,
                    )
                    .map(|previous| elapsed_ms(previous, observation.observed_at_unix_ms))
                    .transpose()?;
                if self.last_capture_completed.len() > maximum {
                    return Err(performance_fatal(
                        "performance_instance_tracking_exhausted",
                        "observe_performance_pipeline_event",
                    ));
                }
                self.record_pipeline_signal(PipelinePerformanceSignal::observed(
                    observation.instance_id,
                    observation.observed_at_unix_ms,
                    frame_gap_ms,
                    Some(capture_latency_ms),
                    None,
                    None,
                )?)
            }
            EventType::CaptureFailed => {
                let frame_id = required_frame_id(&observation)?;
                let pending = take_pending(&mut self.capture_starts, &frame_id)?;
                validate_pending_instance(&pending, &observation.instance_id)?;
                let capture_latency_ms =
                    elapsed_ms(pending.started_at_unix_ms, observation.observed_at_unix_ms)?;
                self.record_pipeline_signal(PipelinePerformanceSignal::observed(
                    observation.instance_id,
                    observation.observed_at_unix_ms,
                    None,
                    Some(capture_latency_ms),
                    None,
                    None,
                )?)
            }
            EventType::RecognitionRequested => {
                let recognition_id = required_recognition_id(&observation)?;
                let frame_id = required_frame_id(&observation)?;
                insert_pending(
                    &mut self.recognition_starts,
                    recognition_id,
                    PendingPipelineMeasurement {
                        instance_id: observation.instance_id,
                        started_at_unix_ms: observation.observed_at_unix_ms,
                        frame_id: Some(frame_id),
                    },
                    maximum,
                )?;
                Ok(Vec::new())
            }
            EventType::RecognitionCompleted => {
                let recognition_id = required_recognition_id(&observation)?;
                let frame_id = required_frame_id(&observation)?;
                let pending = take_pending(&mut self.recognition_starts, &recognition_id)?;
                validate_pending_instance(&pending, &observation.instance_id)?;
                if pending.frame_id != Some(frame_id) {
                    return Err(performance_fatal(
                        "performance_pipeline_identity_conflict",
                        "observe_performance_pipeline_event",
                    ));
                }
                let recognition_latency_ms =
                    elapsed_ms(pending.started_at_unix_ms, observation.observed_at_unix_ms)?;
                self.record_pipeline_signal(PipelinePerformanceSignal::observed(
                    observation.instance_id,
                    observation.observed_at_unix_ms,
                    None,
                    None,
                    Some(recognition_latency_ms),
                    None,
                )?)
            }
            EventType::RecognitionFailed => {
                let recognition_id = required_recognition_id(&observation)?;
                let pending = take_pending(&mut self.recognition_starts, &recognition_id)?;
                validate_pending_instance(&pending, &observation.instance_id)?;
                let recognition_latency_ms =
                    elapsed_ms(pending.started_at_unix_ms, observation.observed_at_unix_ms)?;
                self.record_pipeline_signal(PipelinePerformanceSignal::observed(
                    observation.instance_id,
                    observation.observed_at_unix_ms,
                    None,
                    None,
                    Some(recognition_latency_ms),
                    None,
                )?)
            }
            EventType::TaskEffectIntent => {
                let action_id = required_action_id(&observation)?;
                insert_pending(
                    &mut self.action_starts,
                    action_id,
                    PendingPipelineMeasurement {
                        instance_id: observation.instance_id,
                        started_at_unix_ms: observation.observed_at_unix_ms,
                        frame_id: observation.frame_id,
                    },
                    maximum,
                )?;
                Ok(Vec::new())
            }
            EventType::TaskEffectCompleted => {
                let action_id = required_action_id(&observation)?;
                let pending = take_pending(&mut self.action_starts, &action_id)?;
                validate_pending_instance(&pending, &observation.instance_id)?;
                let action_effect_latency_ms =
                    elapsed_ms(pending.started_at_unix_ms, observation.observed_at_unix_ms)?;
                self.record_pipeline_signal(PipelinePerformanceSignal::observed(
                    observation.instance_id,
                    observation.observed_at_unix_ms,
                    None,
                    None,
                    None,
                    Some(action_effect_latency_ms),
                )?)
            }
            EventType::TaskStepFinished => {
                if let Some(action_id) = observation.action_id {
                    self.action_starts.remove(&action_id);
                }
                Ok(Vec::new())
            }
            EventType::TaskCompleted | EventType::TaskFailed | EventType::TaskCancelled => {
                self.capture_starts
                    .retain(|_, pending| pending.instance_id != observation.instance_id);
                self.recognition_starts
                    .retain(|_, pending| pending.instance_id != observation.instance_id);
                self.action_starts
                    .retain(|_, pending| pending.instance_id != observation.instance_id);
                Ok(Vec::new())
            }
            _ => Ok(Vec::new()),
        }
    }

    pub(crate) fn context(
        &self,
        instance_id: &str,
        window_end_unix_ms: u64,
    ) -> RuntimeHostResult<PerformanceContext> {
        if instance_id.is_empty() || window_end_unix_ms == 0 {
            return Err(performance_fatal(
                "performance_context_query_invalid",
                "read_performance_context",
            ));
        }
        let Some(config) = self.config.as_ref() else {
            return Ok(PerformanceContext::unavailable(window_end_unix_ms));
        };
        let window_ms = duration_ms(config.context_window)?;
        let window_start_unix_ms = window_end_unix_ms.saturating_sub(window_ms).max(1);
        let samples = self
            .system_samples
            .iter()
            .filter(|sample| {
                (window_start_unix_ms..=window_end_unix_ms).contains(&sample.observed_at_unix_ms)
            })
            .collect::<Vec<_>>();
        let pipeline = self
            .pipeline_samples
            .iter()
            .filter(|sample| {
                sample.instance_id == instance_id
                    && (window_start_unix_ms..=window_end_unix_ms)
                        .contains(&sample.observed_at_unix_ms)
            })
            .collect::<Vec<_>>();
        let related_event_ids = self
            .event_references
            .iter()
            .filter(|reference| {
                (window_start_unix_ms..=window_end_unix_ms).contains(&reference.observed_at_unix_ms)
            })
            .map(|reference| reference.event_id)
            .chain(
                self.active_pressures
                    .values()
                    .filter_map(|pressure| pressure.event_id),
            )
            .collect::<BTreeSet<_>>()
            .into_iter()
            .take(128)
            .collect::<Vec<_>>();
        if samples.is_empty() && pipeline.is_empty() && related_event_ids.is_empty() {
            return Ok(PerformanceContext::unavailable(window_end_unix_ms));
        }
        let unavailable_metrics = if samples.is_empty() {
            all_metrics()
        } else {
            samples
                .iter()
                .flat_map(|sample| sample.unavailable_metrics.iter().copied())
                .chain(self.degraded_metrics.iter().copied())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>()
        };
        let health = match self.health {
            _ if samples.is_empty() => PerformanceMonitorHealth::Unavailable,
            PerformanceMonitorHealth::Degraded => PerformanceMonitorHealth::Degraded,
            _ if unavailable_metrics.is_empty() => PerformanceMonitorHealth::Healthy,
            _ => PerformanceMonitorHealth::Partial,
        };
        let pressures = summarize_pressures(&samples, &config.thresholds)?;
        let mut disk_queues = samples
            .iter()
            .filter_map(|sample| sample.disk_queue_depth_milli)
            .collect::<Vec<_>>();
        let mut disk_latencies = samples
            .iter()
            .filter_map(|sample| sample.disk_latency_micros)
            .collect::<Vec<_>>();
        let context = PerformanceContext {
            window_start_unix_ms,
            window_end_unix_ms,
            health,
            sample_count: u32::try_from(samples.len()).map_err(|_| {
                performance_fatal(
                    "performance_sample_count_overflow",
                    "read_performance_context",
                )
            })?,
            unavailable_metrics,
            pressures,
            max_cpu_basis_points: samples
                .iter()
                .map(|sample| sample.cpu_total_basis_points)
                .max(),
            max_ram_basis_points: samples
                .iter()
                .map(|sample| sample.ram_used_basis_points)
                .max(),
            disk_queue_depth_p95_milli: percentile_95(&mut disk_queues),
            disk_latency_p95_micros: percentile_95(&mut disk_latencies),
            max_gpu_basis_points: samples
                .iter()
                .filter_map(|sample| sample.gpu_basis_points)
                .max(),
            max_frame_gap_ms: pipeline
                .iter()
                .filter_map(|sample| sample.frame_gap_ms)
                .max(),
            max_capture_latency_ms: pipeline
                .iter()
                .filter_map(|sample| sample.capture_latency_ms)
                .max(),
            max_recognition_latency_ms: pipeline
                .iter()
                .filter_map(|sample| sample.recognition_latency_ms)
                .max(),
            max_action_effect_latency_ms: pipeline
                .iter()
                .filter_map(|sample| sample.action_effect_latency_ms)
                .max(),
            related_event_ids,
        };
        context.validate().map_err(|_| {
            performance_fatal("performance_context_invalid", "read_performance_context")
        })?;
        Ok(context)
    }

    pub(crate) fn control_observation(
        &mut self,
        observed_at_unix_ms: u64,
    ) -> RuntimeHostResult<Option<PerformanceControlObservation>> {
        if observed_at_unix_ms == 0 {
            return Err(performance_fatal(
                "performance_control_time_invalid",
                "read_performance_control_observation",
            ));
        }
        let config = self.config.as_ref().ok_or_else(|| {
            performance_fatal(
                "performance_monitor_disabled",
                "read_performance_control_observation",
            )
        })?;
        let freshness_ms = duration_ms(config.sample_interval)?
            .checked_mul(2)
            .ok_or_else(|| {
                performance_fatal(
                    "performance_control_duration_overflow",
                    "read_performance_control_observation",
                )
            })?;
        if self
            .system_samples
            .back()
            .is_some_and(|sample| sample.observed_at_unix_ms > observed_at_unix_ms)
            || self
                .pipeline_samples
                .back()
                .is_some_and(|sample| sample.observed_at_unix_ms > observed_at_unix_ms)
        {
            return Err(performance_fatal(
                "performance_control_sample_time_invalid",
                "read_performance_control_observation",
            ));
        }
        let last_sample = self.last_control_sample_unix_ms;
        let latest_system = self.system_samples.back().filter(|sample| {
            is_fresh_control_sample(
                sample.observed_at_unix_ms,
                observed_at_unix_ms,
                last_sample,
                freshness_ms,
            )
        });
        let pipeline = self
            .pipeline_samples
            .iter()
            .rev()
            .take(64)
            .filter(|sample| {
                is_fresh_control_sample(
                    sample.observed_at_unix_ms,
                    observed_at_unix_ms,
                    last_sample,
                    freshness_ms,
                )
            });
        let responsiveness = pipeline
            .filter_map(pipeline_responsiveness_basis_points)
            .min();
        let third_party_pressure = latest_system.and_then(|sample| {
            let process_metrics_available = !sample.unavailable_metrics.iter().any(|metric| {
                matches!(
                    metric,
                    PerformanceMetric::ProcessCpu | PerformanceMetric::ProcessIo
                )
            });
            process_metrics_available.then(|| third_party_pressure_basis_points(sample))
        });
        if responsiveness.is_none() && third_party_pressure.is_none() {
            return Ok(None);
        }
        let source_sample_unix_ms = latest_system
            .map(|sample| sample.observed_at_unix_ms)
            .into_iter()
            .chain(
                self.pipeline_samples
                    .iter()
                    .rev()
                    .take(64)
                    .filter(|sample| {
                        is_fresh_control_sample(
                            sample.observed_at_unix_ms,
                            observed_at_unix_ms,
                            last_sample,
                            freshness_ms,
                        ) && pipeline_responsiveness_basis_points(sample).is_some()
                    })
                    .map(|sample| sample.observed_at_unix_ms),
            )
            .max()
            .ok_or_else(|| {
                performance_fatal(
                    "performance_control_sample_missing",
                    "read_performance_control_observation",
                )
            })?;
        self.last_control_sample_unix_ms = Some(source_sample_unix_ms);
        Ok(Some(PerformanceControlObservation {
            observed_at_unix_ms: source_sample_unix_ms,
            host_responsiveness_basis_points: responsiveness,
            third_party_pressure_basis_points: third_party_pressure,
            foreground_fullscreen: latest_system
                .and_then(|sample| sample.foreground.as_ref())
                .is_some_and(|foreground| foreground.fullscreen),
        }))
    }

    pub(crate) fn record_event_reference(
        &mut self,
        event: &PerformanceSemanticEvent,
        event_id: EventId,
    ) -> RuntimeHostResult<()> {
        let observed_at_unix_ms = event.observed_at_unix_ms();
        self.record_related_event(observed_at_unix_ms, event_id)?;
        if let PerformanceSemanticEvent::PressureStarted(data) = event
            && let Some(active) = self.active_pressures.get_mut(&data.pressure.kind)
        {
            active.event_id = Some(event_id);
        }
        Ok(())
    }

    pub(crate) fn record_monitor_failure(
        &mut self,
        observed_at_unix_ms: u64,
        code: &'static str,
        related_event_id: Option<EventId>,
    ) -> RuntimeHostResult<PerformanceTick> {
        if let Some(event_id) = related_event_id {
            self.record_related_event(observed_at_unix_ms, event_id)?;
        }
        self.ingest_monitor_failure(
            observed_at_unix_ms,
            code,
            pipeline_metrics(),
            MonitorFailureDomain::Pipeline,
        )
    }

    pub(crate) fn record_pipeline_success(
        &mut self,
        observed_at_unix_ms: u64,
    ) -> RuntimeHostResult<Vec<PerformanceSemanticEvent>> {
        if self.pipeline_stopped || self.consecutive_pipeline_failures == 0 {
            return Ok(Vec::new());
        }
        self.consecutive_pipeline_failures = 0;
        for metric in pipeline_metrics() {
            self.degraded_metrics.remove(&metric);
        }
        if self.sampling_stopped
            || self.consecutive_sampler_failures > 0
            || !self.degraded_metrics.is_empty()
        {
            return Ok(Vec::new());
        }
        let Some(sample) = self.system_samples.back() else {
            return Ok(Vec::new());
        };
        self.health = sample_health(sample);
        Ok(vec![PerformanceSemanticEvent::MonitorRecovered(
            PerformanceMonitorStateEventData {
                observed_at_unix_ms,
                health: self.health,
                failure_code: None,
                consecutive_failures: 0,
                terminal: false,
                unavailable_metrics: sample.unavailable_metrics.clone(),
            },
        )])
    }

    fn record_related_event(
        &mut self,
        observed_at_unix_ms: u64,
        event_id: EventId,
    ) -> RuntimeHostResult<()> {
        let config = self.config.as_ref().ok_or_else(|| {
            performance_fatal(
                "performance_monitor_disabled",
                "record_performance_event_reference",
            )
        })?;
        self.event_references.push_back(TimedEventReference {
            observed_at_unix_ms,
            event_id,
        });
        while self.event_references.len() > config.max_event_references {
            self.event_references.pop_front();
        }
        Ok(())
    }

    fn ingest_system_sample(
        &mut self,
        sample: RawSystemSample,
    ) -> RuntimeHostResult<PerformanceTick> {
        let config = self.config.clone().ok_or_else(|| {
            performance_fatal("performance_monitor_disabled", "ingest_performance_sample")
        })?;
        validate_raw_sample(&sample, &config)?;
        if self
            .system_samples
            .back()
            .is_some_and(|previous| previous.observed_at_unix_ms >= sample.observed_at_unix_ms)
        {
            return Err(performance_fatal(
                "performance_sample_time_invalid",
                "ingest_performance_sample",
            ));
        }
        let previous_health = self.health;
        self.consecutive_sampler_failures = 0;
        for metric in host_metrics() {
            self.degraded_metrics.remove(&metric);
        }
        self.health = if self.pipeline_stopped
            || self.consecutive_pipeline_failures > 0
            || !self.degraded_metrics.is_empty()
        {
            PerformanceMonitorHealth::Degraded
        } else {
            sample_health(&sample)
        };
        self.system_samples.push_back(sample.clone());
        trim_queue(
            &mut self.system_samples,
            config.max_system_samples,
            |value| value.observed_at_unix_ms,
            config.context_window,
        )?;
        let mut events = self.update_pressures(&sample)?;
        if previous_health == PerformanceMonitorHealth::Degraded
            && self.health != PerformanceMonitorHealth::Degraded
        {
            events.push(PerformanceSemanticEvent::MonitorRecovered(
                PerformanceMonitorStateEventData {
                    observed_at_unix_ms: sample.observed_at_unix_ms,
                    health: self.health,
                    failure_code: None,
                    consecutive_failures: 0,
                    terminal: false,
                    unavailable_metrics: sample.unavailable_metrics.clone(),
                },
            ));
        }
        let summary_interval_ms = duration_ms(config.summary_interval)?;
        let summary_due = self.last_summary_unix_ms.is_none_or(|previous| {
            sample.observed_at_unix_ms.saturating_sub(previous) >= summary_interval_ms
        });
        if summary_due {
            let instance_id = "runtime";
            let context = self.context(instance_id, sample.observed_at_unix_ms)?;
            events.push(PerformanceSemanticEvent::Summary(Box::new(
                PerformanceSummaryEventData {
                    context,
                    foreground: sample.foreground.clone(),
                    owned_processes: sample.owned_processes.iter().map(process_summary).collect(),
                    third_party_high_load: sample
                        .third_party_high_load
                        .iter()
                        .map(process_summary)
                        .collect(),
                },
            )));
            self.last_summary_unix_ms = Some(sample.observed_at_unix_ms);
        }
        Ok(PerformanceTick {
            events,
            stop_sampling: false,
        })
    }

    fn ingest_monitor_failure(
        &mut self,
        observed_at_unix_ms: u64,
        code: &'static str,
        unavailable_metrics: Vec<PerformanceMetric>,
        domain: MonitorFailureDomain,
    ) -> RuntimeHostResult<PerformanceTick> {
        let config = self.config.as_ref().ok_or_else(|| {
            performance_fatal(
                "performance_monitor_disabled",
                "record_performance_monitor_failure",
            )
        })?;
        if observed_at_unix_ms == 0
            || !valid_error_code(code)
            || unavailable_metrics.is_empty()
            || unavailable_metrics
                .iter()
                .copied()
                .collect::<BTreeSet<_>>()
                .len()
                != unavailable_metrics.len()
        {
            return Err(performance_fatal(
                "performance_monitor_failure_invalid",
                "record_performance_monitor_failure",
            ));
        }
        let consecutive_failures = match domain {
            MonitorFailureDomain::Sampler => &mut self.consecutive_sampler_failures,
            MonitorFailureDomain::Pipeline => &mut self.consecutive_pipeline_failures,
        };
        *consecutive_failures = consecutive_failures.checked_add(1).ok_or_else(|| {
            performance_fatal(
                "performance_failure_count_overflow",
                "record_performance_monitor_failure",
            )
        })?;
        self.health = PerformanceMonitorHealth::Degraded;
        self.degraded_metrics
            .extend(unavailable_metrics.iter().copied());
        let terminal = *consecutive_failures >= config.max_consecutive_failures;
        if terminal {
            match domain {
                MonitorFailureDomain::Sampler => self.sampling_stopped = true,
                MonitorFailureDomain::Pipeline => self.pipeline_stopped = true,
            }
        }
        let events = if *consecutive_failures == 1 || terminal {
            vec![PerformanceSemanticEvent::MonitorDegraded(
                PerformanceMonitorStateEventData {
                    observed_at_unix_ms,
                    health: PerformanceMonitorHealth::Degraded,
                    failure_code: Some(code.to_owned()),
                    consecutive_failures: *consecutive_failures,
                    terminal,
                    unavailable_metrics,
                },
            )]
        } else {
            Vec::new()
        };
        Ok(PerformanceTick {
            events,
            stop_sampling: terminal && matches!(domain, MonitorFailureDomain::Sampler),
        })
    }

    fn update_pressures(
        &mut self,
        sample: &RawSystemSample,
    ) -> RuntimeHostResult<Vec<PerformanceSemanticEvent>> {
        let config = self.config.as_ref().ok_or_else(|| {
            performance_fatal(
                "performance_monitor_disabled",
                "update_performance_pressures",
            )
        })?;
        let measurements = pressure_measurements(sample)?;
        let mut events = Vec::new();
        for kind in [
            PerformancePressureKind::Cpu,
            PerformancePressureKind::Ram,
            PerformancePressureKind::DiskIo,
            PerformancePressureKind::Gpu,
            PerformancePressureKind::ThirdParty,
        ] {
            let measurement = measurements.get(&kind);
            let remains_active = self.active_pressures.contains_key(&kind)
                && measurement
                    .is_some_and(|value| pressure_above_end(kind, value, &config.thresholds));
            if let (Some(active), Some(value)) = (self.active_pressures.get_mut(&kind), measurement)
                && remains_active
            {
                active.record.last_observed_at_unix_ms = sample.observed_at_unix_ms;
                active.record.severity = pressure_severity(kind, value, &config.thresholds);
                active.record.peak = peak_value(&active.record.peak, value);
                continue;
            }
            if let Some(mut ended) = self.active_pressures.remove(&kind) {
                ended.record.last_observed_at_unix_ms = sample.observed_at_unix_ms;
                events.push(PerformanceSemanticEvent::PressureEnded(
                    PerformancePressureEventData {
                        observed_at_unix_ms: sample.observed_at_unix_ms,
                        pressure: ended.record,
                    },
                ));
            }
            if let Some(value) = measurement
                && pressure_above_start(kind, value, &config.thresholds)
            {
                let record = PerformancePressureRecord {
                    kind,
                    severity: pressure_severity(kind, value, &config.thresholds),
                    started_at_unix_ms: sample.observed_at_unix_ms,
                    last_observed_at_unix_ms: sample.observed_at_unix_ms,
                    peak: value.clone(),
                };
                self.active_pressures.insert(
                    kind,
                    ActivePressure {
                        record: record.clone(),
                        event_id: None,
                    },
                );
                events.push(PerformanceSemanticEvent::PressureStarted(
                    PerformancePressureEventData {
                        observed_at_unix_ms: sample.observed_at_unix_ms,
                        pressure: record,
                    },
                ));
            }
        }
        Ok(events)
    }
}

fn validate_pipeline_observation(observation: &PipelineEventObservation) -> RuntimeHostResult<()> {
    if observation.instance_id.is_empty()
        || observation.instance_id.len() > 128
        || observation.instance_id.chars().any(char::is_control)
        || observation.observed_at_unix_ms == 0
    {
        return Err(performance_fatal(
            "performance_pipeline_observation_invalid",
            "observe_performance_pipeline_event",
        ));
    }
    Ok(())
}

fn sample_health(sample: &RawSystemSample) -> PerformanceMonitorHealth {
    if sample.unavailable_metrics.is_empty() {
        PerformanceMonitorHealth::Healthy
    } else {
        PerformanceMonitorHealth::Partial
    }
}

fn required_frame_id(observation: &PipelineEventObservation) -> RuntimeHostResult<FrameId> {
    observation.frame_id.ok_or_else(|| {
        performance_fatal(
            "performance_pipeline_frame_missing",
            "observe_performance_pipeline_event",
        )
    })
}

fn required_recognition_id(
    observation: &PipelineEventObservation,
) -> RuntimeHostResult<RecognitionId> {
    observation.recognition_id.ok_or_else(|| {
        performance_fatal(
            "performance_pipeline_recognition_missing",
            "observe_performance_pipeline_event",
        )
    })
}

fn required_action_id(observation: &PipelineEventObservation) -> RuntimeHostResult<ActionId> {
    observation.action_id.ok_or_else(|| {
        performance_fatal(
            "performance_pipeline_action_missing",
            "observe_performance_pipeline_event",
        )
    })
}

fn insert_pending<K: Ord + Copy>(
    pending: &mut BTreeMap<K, PendingPipelineMeasurement>,
    key: K,
    value: PendingPipelineMeasurement,
    maximum: usize,
) -> RuntimeHostResult<()> {
    if pending.contains_key(&key) {
        return Err(performance_fatal(
            "performance_pipeline_identity_reused",
            "observe_performance_pipeline_event",
        ));
    }
    if pending.len() >= maximum {
        return Err(performance_fatal(
            "performance_pipeline_tracking_exhausted",
            "observe_performance_pipeline_event",
        ));
    }
    pending.insert(key, value);
    Ok(())
}

fn take_pending<K: Ord>(
    pending: &mut BTreeMap<K, PendingPipelineMeasurement>,
    key: &K,
) -> RuntimeHostResult<PendingPipelineMeasurement> {
    pending.remove(key).ok_or_else(|| {
        performance_fatal(
            "performance_pipeline_start_missing",
            "observe_performance_pipeline_event",
        )
    })
}

fn validate_pending_instance(
    pending: &PendingPipelineMeasurement,
    instance_id: &str,
) -> RuntimeHostResult<()> {
    if pending.instance_id != instance_id {
        return Err(performance_fatal(
            "performance_pipeline_identity_conflict",
            "observe_performance_pipeline_event",
        ));
    }
    Ok(())
}

fn elapsed_ms(started_at_unix_ms: u64, observed_at_unix_ms: u64) -> RuntimeHostResult<u64> {
    observed_at_unix_ms
        .checked_sub(started_at_unix_ms)
        .ok_or_else(|| {
            performance_fatal(
                "performance_pipeline_time_invalid",
                "observe_performance_pipeline_event",
            )
        })
}

fn summarize_pressures(
    samples: &[&RawSystemSample],
    thresholds: &PerformanceThresholds,
) -> RuntimeHostResult<Vec<PerformancePressureRecord>> {
    let mut pressures = BTreeMap::<PerformancePressureKind, PerformancePressureRecord>::new();
    for sample in samples {
        for (kind, value) in pressure_measurements(sample)? {
            if !pressure_above_start(kind, &value, thresholds) {
                continue;
            }
            pressures
                .entry(kind)
                .and_modify(|record| {
                    record.last_observed_at_unix_ms = sample.observed_at_unix_ms;
                    record.severity = record
                        .severity
                        .max(pressure_severity(kind, &value, thresholds));
                    record.peak = peak_value(&record.peak, &value);
                })
                .or_insert_with(|| PerformancePressureRecord {
                    kind,
                    severity: pressure_severity(kind, &value, thresholds),
                    started_at_unix_ms: sample.observed_at_unix_ms,
                    last_observed_at_unix_ms: sample.observed_at_unix_ms,
                    peak: value,
                });
        }
    }
    Ok(pressures.into_values().collect())
}

fn pressure_measurements(
    sample: &RawSystemSample,
) -> RuntimeHostResult<BTreeMap<PerformancePressureKind, PerformancePressureValue>> {
    let mut values = BTreeMap::from([
        (
            PerformancePressureKind::Cpu,
            PerformancePressureValue::Utilization {
                basis_points: sample.cpu_total_basis_points,
            },
        ),
        (
            PerformancePressureKind::Ram,
            PerformancePressureValue::Utilization {
                basis_points: sample.ram_used_basis_points,
            },
        ),
    ]);
    if sample.disk_queue_depth_milli.is_some() || sample.disk_latency_micros.is_some() {
        values.insert(
            PerformancePressureKind::DiskIo,
            PerformancePressureValue::DiskIo {
                queue_depth_milli: sample.disk_queue_depth_milli.unwrap_or(0),
                latency_micros: sample.disk_latency_micros.unwrap_or(0),
            },
        );
    }
    if let Some(basis_points) = sample.gpu_basis_points {
        values.insert(
            PerformancePressureKind::Gpu,
            PerformancePressureValue::Utilization { basis_points },
        );
    }
    if !sample.third_party_high_load.is_empty() {
        let process_count = u16::try_from(sample.third_party_high_load.len()).map_err(|_| {
            performance_fatal(
                "performance_process_count_overflow",
                "measure_performance_pressure",
            )
        })?;
        let peak_cpu_basis_points = sample
            .third_party_high_load
            .iter()
            .map(|process| process.cpu_basis_points)
            .max()
            .ok_or_else(|| {
                performance_fatal(
                    "performance_process_sample_missing",
                    "measure_performance_pressure",
                )
            })?;
        let peak_io_bytes_per_second = sample
            .third_party_high_load
            .iter()
            .map(|process| process.io_bytes_per_second)
            .max()
            .ok_or_else(|| {
                performance_fatal(
                    "performance_process_sample_missing",
                    "measure_performance_pressure",
                )
            })?;
        values.insert(
            PerformancePressureKind::ThirdParty,
            PerformancePressureValue::ProcessLoad {
                process_count,
                peak_cpu_basis_points,
                peak_io_bytes_per_second,
            },
        );
    }
    Ok(values)
}

fn pipeline_responsiveness_basis_points(signal: &PipelinePerformanceSignal) -> Option<u16> {
    [
        signal.frame_gap_ms.map(|value| latency_score(value, 1_000)),
        signal
            .capture_latency_ms
            .map(|value| latency_score(value, 500)),
        signal
            .recognition_latency_ms
            .map(|value| latency_score(value, 1_000)),
        signal
            .action_effect_latency_ms
            .map(|value| latency_score(value, 1_500)),
    ]
    .into_iter()
    .flatten()
    .min()
}

fn latency_score(actual_ms: u64, target_ms: u64) -> u16 {
    if actual_ms <= target_ms {
        return BASIS_POINTS_MAX;
    }
    u16::try_from(target_ms.saturating_mul(u64::from(BASIS_POINTS_MAX)) / actual_ms).unwrap_or(0)
}

fn third_party_pressure_basis_points(sample: &RawSystemSample) -> u16 {
    let cpu = sample
        .third_party_high_load
        .iter()
        .map(|process| process.cpu_basis_points)
        .max()
        .unwrap_or(0);
    let io = sample
        .third_party_high_load
        .iter()
        .map(|process| {
            process
                .io_bytes_per_second
                .saturating_mul(u64::from(BASIS_POINTS_MAX))
                / (64 * 1024 * 1024)
        })
        .max()
        .unwrap_or(0)
        .min(u64::from(BASIS_POINTS_MAX));
    cpu.max(u16::try_from(io).unwrap_or(BASIS_POINTS_MAX))
}

fn pressure_above_start(
    kind: PerformancePressureKind,
    value: &PerformancePressureValue,
    thresholds: &PerformanceThresholds,
) -> bool {
    match (kind, value) {
        (PerformancePressureKind::Cpu, PerformancePressureValue::Utilization { basis_points }) => {
            *basis_points >= thresholds.cpu_start_basis_points
        }
        (PerformancePressureKind::Ram, PerformancePressureValue::Utilization { basis_points }) => {
            *basis_points >= thresholds.ram_start_basis_points
        }
        (PerformancePressureKind::Gpu, PerformancePressureValue::Utilization { basis_points }) => {
            *basis_points >= thresholds.gpu_start_basis_points
        }
        (
            PerformancePressureKind::DiskIo,
            PerformancePressureValue::DiskIo {
                queue_depth_milli,
                latency_micros,
            },
        ) => {
            *queue_depth_milli >= thresholds.disk_queue_start_milli
                || *latency_micros >= thresholds.disk_latency_start_micros
        }
        (PerformancePressureKind::ThirdParty, PerformancePressureValue::ProcessLoad { .. }) => true,
        _ => false,
    }
}

fn pressure_above_end(
    kind: PerformancePressureKind,
    value: &PerformancePressureValue,
    thresholds: &PerformanceThresholds,
) -> bool {
    match (kind, value) {
        (PerformancePressureKind::Cpu, PerformancePressureValue::Utilization { basis_points }) => {
            *basis_points >= thresholds.cpu_end_basis_points
        }
        (PerformancePressureKind::Ram, PerformancePressureValue::Utilization { basis_points }) => {
            *basis_points >= thresholds.ram_end_basis_points
        }
        (PerformancePressureKind::Gpu, PerformancePressureValue::Utilization { basis_points }) => {
            *basis_points >= thresholds.gpu_end_basis_points
        }
        (
            PerformancePressureKind::DiskIo,
            PerformancePressureValue::DiskIo {
                queue_depth_milli,
                latency_micros,
            },
        ) => {
            *queue_depth_milli >= thresholds.disk_queue_end_milli
                || *latency_micros >= thresholds.disk_latency_end_micros
        }
        (PerformancePressureKind::ThirdParty, PerformancePressureValue::ProcessLoad { .. }) => true,
        _ => false,
    }
}

fn pressure_severity(
    kind: PerformancePressureKind,
    value: &PerformancePressureValue,
    thresholds: &PerformanceThresholds,
) -> PerformancePressureSeverity {
    let critical = match (kind, value) {
        (
            PerformancePressureKind::Cpu
            | PerformancePressureKind::Ram
            | PerformancePressureKind::Gpu,
            PerformancePressureValue::Utilization { basis_points },
        ) => *basis_points >= 9_700,
        (
            PerformancePressureKind::DiskIo,
            PerformancePressureValue::DiskIo {
                queue_depth_milli,
                latency_micros,
            },
        ) => {
            *queue_depth_milli >= thresholds.disk_queue_start_milli.saturating_mul(2)
                || *latency_micros >= thresholds.disk_latency_start_micros.saturating_mul(2)
        }
        (
            PerformancePressureKind::ThirdParty,
            PerformancePressureValue::ProcessLoad {
                process_count,
                peak_cpu_basis_points,
                ..
            },
        ) => *process_count >= 3 || *peak_cpu_basis_points >= 8_000,
        _ => false,
    };
    if critical {
        PerformancePressureSeverity::Critical
    } else {
        let high = match value {
            PerformancePressureValue::Utilization { basis_points } => *basis_points >= 9_200,
            PerformancePressureValue::DiskIo {
                queue_depth_milli,
                latency_micros,
            } => {
                *queue_depth_milli >= thresholds.disk_queue_start_milli.saturating_add(2_000)
                    || *latency_micros
                        >= thresholds.disk_latency_start_micros.saturating_add(25_000)
            }
            PerformancePressureValue::ProcessLoad {
                process_count,
                peak_cpu_basis_points,
                ..
            } => *process_count >= 2 || *peak_cpu_basis_points >= 6_000,
        };
        if high {
            PerformancePressureSeverity::High
        } else {
            PerformancePressureSeverity::Elevated
        }
    }
}

fn peak_value(
    current: &PerformancePressureValue,
    candidate: &PerformancePressureValue,
) -> PerformancePressureValue {
    match (current, candidate) {
        (
            PerformancePressureValue::Utilization {
                basis_points: current,
            },
            PerformancePressureValue::Utilization {
                basis_points: candidate,
            },
        ) => PerformancePressureValue::Utilization {
            basis_points: (*current).max(*candidate),
        },
        (
            PerformancePressureValue::DiskIo {
                queue_depth_milli: current_queue,
                latency_micros: current_latency,
            },
            PerformancePressureValue::DiskIo {
                queue_depth_milli: candidate_queue,
                latency_micros: candidate_latency,
            },
        ) => PerformancePressureValue::DiskIo {
            queue_depth_milli: (*current_queue).max(*candidate_queue),
            latency_micros: (*current_latency).max(*candidate_latency),
        },
        (
            PerformancePressureValue::ProcessLoad {
                process_count: current_count,
                peak_cpu_basis_points: current_cpu,
                peak_io_bytes_per_second: current_io,
            },
            PerformancePressureValue::ProcessLoad {
                process_count: candidate_count,
                peak_cpu_basis_points: candidate_cpu,
                peak_io_bytes_per_second: candidate_io,
            },
        ) => PerformancePressureValue::ProcessLoad {
            process_count: (*current_count).max(*candidate_count),
            peak_cpu_basis_points: (*current_cpu).max(*candidate_cpu),
            peak_io_bytes_per_second: (*current_io).max(*candidate_io),
        },
        _ => candidate.clone(),
    }
}

fn validate_raw_sample(
    sample: &RawSystemSample,
    config: &PerformanceMonitorConfig,
) -> RuntimeHostResult<()> {
    if sample.observed_at_unix_ms == 0
        || sample.cpu_total_basis_points > BASIS_POINTS_MAX
        || sample.ram_used_basis_points > BASIS_POINTS_MAX
        || sample
            .cpu_per_core_basis_points
            .iter()
            .any(|value| *value > BASIS_POINTS_MAX)
        || sample
            .gpu_basis_points
            .is_some_and(|value| value > BASIS_POINTS_MAX)
        || sample.owned_processes.len() > 32
        || sample.third_party_high_load.len() > config.top_process_count
        || sample
            .owned_processes
            .iter()
            .chain(sample.third_party_high_load.iter())
            .any(|process| {
                !valid_process_label(process.pid, &process.process_name)
                    || process.cpu_basis_points > BASIS_POINTS_MAX
            })
    {
        return Err(performance_fatal(
            "performance_sample_invalid",
            "ingest_performance_sample",
        ));
    }
    Ok(())
}

fn process_summary(sample: &RawProcessSample) -> PerformanceProcessSummary {
    PerformanceProcessSummary {
        pid: sample.pid,
        process_name: sample.process_name.clone(),
        ownership: sample.ownership,
        cpu_basis_points: sample.cpu_basis_points,
        working_set_bytes: sample.working_set_bytes,
        io_bytes_per_second: sample.io_bytes_per_second,
    }
}

fn raw_system_sample(sample: HostSample) -> RawSystemSample {
    RawSystemSample {
        observed_at_unix_ms: sample.observed_at_unix_ms,
        cpu_total_basis_points: sample.cpu_total_basis_points,
        cpu_per_core_basis_points: sample.cpu_per_core_basis_points,
        ram_used_basis_points: sample.ram_used_basis_points,
        disk_queue_depth_milli: sample.disk_queue_depth_milli,
        disk_latency_micros: sample.disk_latency_micros,
        gpu_basis_points: sample.gpu_basis_points,
        foreground: sample
            .foreground
            .map(|foreground| PerformanceForegroundSummary {
                process: host_process_summary(foreground.process),
                fullscreen: foreground.fullscreen,
            }),
        owned_processes: sample
            .owned_processes
            .into_iter()
            .map(raw_process_sample)
            .collect(),
        third_party_high_load: sample
            .third_party_high_load
            .into_iter()
            .map(raw_process_sample)
            .collect(),
        unavailable_metrics: sample
            .unavailable_metrics
            .into_iter()
            .map(performance_metric)
            .collect(),
    }
}

fn raw_process_sample(sample: HostProcessSample) -> RawProcessSample {
    RawProcessSample {
        pid: sample.pid,
        process_name: sample.process_name,
        ownership: process_ownership(sample.ownership),
        cpu_basis_points: sample.cpu_basis_points,
        working_set_bytes: sample.working_set_bytes,
        io_bytes_per_second: sample.io_bytes_per_second,
    }
}

fn host_process_summary(sample: HostProcessSample) -> PerformanceProcessSummary {
    PerformanceProcessSummary {
        pid: sample.pid,
        process_name: sample.process_name,
        ownership: process_ownership(sample.ownership),
        cpu_basis_points: sample.cpu_basis_points,
        working_set_bytes: sample.working_set_bytes,
        io_bytes_per_second: sample.io_bytes_per_second,
    }
}

const fn process_ownership(value: HostProcessOwnership) -> PerformanceProcessOwnership {
    match value {
        HostProcessOwnership::Runtime => PerformanceProcessOwnership::Runtime,
        HostProcessOwnership::Owned => PerformanceProcessOwnership::Owned,
        HostProcessOwnership::ThirdParty => PerformanceProcessOwnership::ThirdParty,
    }
}

const fn performance_metric(value: HostMetric) -> PerformanceMetric {
    match value {
        HostMetric::CpuPerCore => PerformanceMetric::CpuPerCore,
        HostMetric::DiskQueueDepth => PerformanceMetric::DiskQueueDepth,
        HostMetric::DiskLatency => PerformanceMetric::DiskLatency,
        HostMetric::Gpu => PerformanceMetric::Gpu,
        HostMetric::ProcessCpu => PerformanceMetric::ProcessCpu,
        HostMetric::ProcessRam => PerformanceMetric::ProcessRam,
        HostMetric::ProcessIo => PerformanceMetric::ProcessIo,
        HostMetric::ForegroundProcess => PerformanceMetric::ForegroundProcess,
    }
}

fn trim_queue<T, F>(
    queue: &mut VecDeque<T>,
    maximum: usize,
    timestamp: F,
    window: Duration,
) -> RuntimeHostResult<()>
where
    F: Fn(&T) -> u64,
{
    let Some(latest) = queue.back().map(&timestamp) else {
        return Ok(());
    };
    let window_ms = duration_ms(window)?;
    let oldest = latest.saturating_sub(window_ms);
    while queue.len() > maximum || queue.front().is_some_and(|value| timestamp(value) < oldest) {
        queue.pop_front();
    }
    Ok(())
}

fn percentile_95<T: Ord + Copy>(values: &mut [T]) -> Option<T> {
    if values.is_empty() {
        return None;
    }
    values.sort_unstable();
    let index = values
        .len()
        .saturating_mul(95)
        .div_ceil(100)
        .saturating_sub(1);
    values.get(index).copied()
}

fn duration_ms(duration: Duration) -> RuntimeHostResult<u64> {
    u64::try_from(duration.as_millis()).map_err(|_| {
        performance_fatal(
            "performance_duration_overflow",
            "convert_performance_duration",
        )
    })
}

fn is_fresh_control_sample(
    sample_unix_ms: u64,
    observed_at_unix_ms: u64,
    last_sample_unix_ms: Option<u64>,
    freshness_ms: u64,
) -> bool {
    sample_unix_ms <= observed_at_unix_ms
        && last_sample_unix_ms.is_none_or(|last| sample_unix_ms > last)
        && observed_at_unix_ms - sample_unix_ms <= freshness_ms
}

fn valid_process_label(pid: u32, label: &str) -> bool {
    pid > 0 && !label.is_empty() && label.len() <= 260 && !label.chars().any(char::is_control)
}

fn valid_error_code(code: &str) -> bool {
    !code.is_empty()
        && code.len() <= 128
        && code
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn all_metrics() -> Vec<PerformanceMetric> {
    host_metrics()
        .into_iter()
        .chain(pipeline_metrics())
        .collect()
}

fn host_metrics() -> Vec<PerformanceMetric> {
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
    ]
}

fn pipeline_metrics() -> Vec<PerformanceMetric> {
    vec![
        PerformanceMetric::FrameGap,
        PerformanceMetric::CaptureLatency,
        PerformanceMetric::RecognitionLatency,
        PerformanceMetric::ActionEffectLatency,
    ]
}

fn performance_fatal(code: &'static str, operation: &'static str) -> RuntimeHostError {
    RuntimeHostError::fatal(code, operation, RuntimeErrorCode::RuntimeFatal)
}

#[cfg(test)]
mod tests {
    use super::*;
    use actingcommand_contract::IdentifierIssuer;

    struct SequenceSampler {
        samples: VecDeque<Result<HostSample, &'static str>>,
    }

    impl HostSampler for SequenceSampler {
        fn sample(
            &mut self,
            _observed_at_unix_ms: u64,
            _owned_processes: &BTreeMap<u32, String>,
            _top_process_count: usize,
            _thresholds: ProcessLoadThresholds,
        ) -> Result<HostSample, &'static str> {
            self.samples
                .pop_front()
                .expect("test sampler sequence is complete")
        }
    }

    fn sample(at: u64, cpu: u16) -> HostSample {
        HostSample {
            observed_at_unix_ms: at,
            cpu_total_basis_points: cpu,
            cpu_per_core_basis_points: vec![cpu],
            ram_used_basis_points: 4_000,
            disk_queue_depth_milli: Some(500),
            disk_latency_micros: Some(1_000),
            gpu_basis_points: None,
            foreground: None,
            owned_processes: Vec::new(),
            third_party_high_load: Vec::new(),
            process_coverage: actingcommand_host_metrics::ProcessMetricCoverage {
                enumerated_processes: 1,
                sampled_processes: 1,
                audited_protected_processes_excluded: 0,
                unconfirmed_access_denied_processes: 0,
                unexpectedly_inaccessible_processes: 0,
                readable_basis_points: 10_000,
            },
            unavailable_metrics: vec![HostMetric::Gpu, HostMetric::ForegroundProcess],
        }
    }

    fn monitor(samples: Vec<Result<HostSample, &'static str>>) -> PerformanceMonitor {
        PerformanceMonitor::enabled(
            PerformanceMonitorConfig::default(),
            Ok(Box::new(SequenceSampler {
                samples: samples.into(),
            })),
        )
        .expect("monitor")
    }

    #[test]
    fn deterministic_pressure_counter_starts_and_ends_once() {
        let mut monitor = monitor(vec![
            Ok(sample(1_000, 9_000)),
            Ok(sample(3_000, 9_100)),
            Ok(sample(5_000, 6_000)),
        ]);
        let first = monitor.tick(1_000).expect("first");
        let second = monitor.tick(3_000).expect("second");
        let third = monitor.tick(5_000).expect("third");
        assert_eq!(
            first
                .events
                .iter()
                .filter(|event| matches!(event, PerformanceSemanticEvent::PressureStarted(_)))
                .count(),
            1
        );
        assert!(!second.events.iter().any(|event| matches!(
            event,
            PerformanceSemanticEvent::PressureStarted(_)
                | PerformanceSemanticEvent::PressureEnded(_)
        )));
        assert_eq!(
            third
                .events
                .iter()
                .filter(|event| matches!(event, PerformanceSemanticEvent::PressureEnded(_)))
                .count(),
            1
        );
    }

    #[test]
    fn owned_process_registry_includes_runtime_within_the_declared_bound() {
        let mut config = PerformanceMonitorConfig::default();
        for offset in 0..MAX_OWNED_PROCESS_COUNT {
            config = config.with_owned_process(
                u32::MAX - u32::try_from(offset).expect("bounded offset"),
                format!("owned-{offset}"),
            );
        }
        let error = match PerformanceMonitor::enabled(
            config,
            Ok(Box::new(SequenceSampler {
                samples: VecDeque::new(),
            })),
        ) {
            Ok(_) => panic!("runtime PID must not overflow the owned-process registry"),
            Err(error) => error,
        };
        assert_eq!(error.code(), "performance_config_invalid");
    }

    #[test]
    fn pressure_injection_and_pipeline_stutter_enter_failure_context() {
        let mut monitor = monitor(vec![Ok(sample(30_000, 9_500))]);
        let pressure = monitor.tick(30_000).expect("sample");
        let event_id = *IdentifierIssuer::new()
            .expect("identifiers")
            .mint_event_id()
            .expect("event")
            .transport();
        monitor
            .record_event_reference(&pressure.events[0], event_id)
            .expect("event reference");
        let signal = PipelinePerformanceSignal::new("fixture-instance", 31_000, 1_500)
            .expect("signal")
            .with_capture_latency(900)
            .expect("capture")
            .with_recognition_latency(700)
            .expect("recognition")
            .with_action_effect_latency(800)
            .expect("action");
        let events = monitor.record_pipeline_signal(signal).expect("record");
        assert!(matches!(
            events.as_slice(),
            [PerformanceSemanticEvent::StutterDetected(_)]
        ));
        let context = monitor
            .context("fixture-instance", 31_000)
            .expect("context");
        assert!(context.pressure_observed());
        assert_eq!(context.max_frame_gap_ms, Some(1_500));
        assert_eq!(context.max_capture_latency_ms, Some(900));
        assert_eq!(context.max_recognition_latency_ms, Some(700));
        assert_eq!(context.max_action_effect_latency_ms, Some(800));
        assert_eq!(context.related_event_ids, vec![event_id]);
    }

    #[test]
    fn pipeline_probes_synthesize_responsiveness_without_faking_missing_samples() {
        let mut empty = monitor(Vec::new());
        assert_eq!(
            empty.control_observation(1_000).expect("empty observation"),
            None
        );

        let mut monitor = monitor(vec![Ok(sample(1_000, 4_000))]);
        monitor.tick(1_000).expect("host sample");
        monitor
            .record_pipeline_signal(
                PipelinePerformanceSignal::new("fixture-instance", 1_500, 2_000)
                    .expect("signal")
                    .with_capture_latency(1_000)
                    .expect("capture"),
            )
            .expect("pipeline sample");
        let observation = monitor
            .control_observation(1_500)
            .expect("observation")
            .expect("measured observation");
        assert_eq!(observation.observed_at_unix_ms, 1_500);
        assert_eq!(observation.host_responsiveness_basis_points, Some(5_000));
        assert_eq!(observation.third_party_pressure_basis_points, Some(0));
        assert_eq!(
            monitor
                .control_observation(1_600)
                .expect("consumed observation"),
            None
        );
    }

    #[test]
    fn incomplete_process_coverage_propagates_unknown_third_party_pressure() {
        let mut denied_heavy = sample(1_000, 4_000);
        denied_heavy.process_coverage = actingcommand_host_metrics::ProcessMetricCoverage {
            enumerated_processes: 100,
            sampled_processes: 1,
            audited_protected_processes_excluded: 0,
            unconfirmed_access_denied_processes: 99,
            unexpectedly_inaccessible_processes: 0,
            readable_basis_points: 100,
        };
        denied_heavy.unavailable_metrics.extend([
            HostMetric::ProcessCpu,
            HostMetric::ProcessRam,
            HostMetric::ProcessIo,
        ]);
        let mut monitor = monitor(vec![Ok(denied_heavy)]);
        monitor.tick(1_000).expect("host sample");
        monitor
            .record_pipeline_signal(
                PipelinePerformanceSignal::new("fixture-instance", 1_500, 2_000)
                    .expect("pipeline signal"),
            )
            .expect("pipeline sample");

        let observation = monitor
            .control_observation(1_500)
            .expect("observation")
            .expect("responsiveness observation");
        assert_eq!(observation.host_responsiveness_basis_points, Some(5_000));
        assert_eq!(observation.third_party_pressure_basis_points, None);
    }

    #[test]
    fn sampler_failures_do_not_relabel_old_control_evidence_as_fresh() {
        let mut monitor = monitor(vec![
            Ok(sample(1_000, 4_000)),
            Err("performance_counter_failed"),
            Err("performance_counter_failed"),
        ]);
        monitor.tick(1_000).expect("host sample");
        monitor
            .record_pipeline_signal(
                PipelinePerformanceSignal::new("fixture-instance", 1_500, 2_000)
                    .expect("pipeline sample"),
            )
            .expect("pipeline sample");
        assert_eq!(
            monitor
                .control_observation(1_500)
                .expect("fresh observation")
                .expect("fresh observation")
                .observed_at_unix_ms,
            1_500
        );

        monitor.tick(3_000).expect("first sampler failure");
        assert_eq!(
            monitor
                .control_observation(3_000)
                .expect("first degraded observation"),
            None
        );
        monitor.tick(5_000).expect("second sampler failure");
        assert_eq!(
            monitor
                .control_observation(5_000)
                .expect("second degraded observation"),
            None
        );
    }

    #[test]
    fn typed_pipeline_events_collect_capture_recognition_and_effect_latency() {
        let mut monitor = monitor(Vec::new());
        let identifiers = IdentifierIssuer::new().expect("identifiers");
        let first_frame = *identifiers.mint_frame_id().expect("frame").transport();
        let second_frame = *identifiers.mint_frame_id().expect("frame").transport();
        let recognition_id = *identifiers
            .mint_recognition_id()
            .expect("recognition")
            .transport();
        let action_id = *identifiers.mint_action_id().expect("action").transport();

        for observation in [
            PipelineEventObservation {
                event_type: EventType::CaptureRequested,
                instance_id: "fixture-instance".to_owned(),
                observed_at_unix_ms: 1_000,
                frame_id: Some(first_frame),
                recognition_id: None,
                action_id: None,
            },
            PipelineEventObservation {
                event_type: EventType::CaptureCompleted,
                instance_id: "fixture-instance".to_owned(),
                observed_at_unix_ms: 1_100,
                frame_id: Some(first_frame),
                recognition_id: None,
                action_id: None,
            },
            PipelineEventObservation {
                event_type: EventType::CaptureRequested,
                instance_id: "fixture-instance".to_owned(),
                observed_at_unix_ms: 1_200,
                frame_id: Some(second_frame),
                recognition_id: None,
                action_id: None,
            },
        ] {
            assert!(
                monitor
                    .observe_pipeline_event(observation)
                    .expect("pipeline event")
                    .is_empty()
            );
        }
        let capture_events = monitor
            .observe_pipeline_event(PipelineEventObservation {
                event_type: EventType::CaptureCompleted,
                instance_id: "fixture-instance".to_owned(),
                observed_at_unix_ms: 2_500,
                frame_id: Some(second_frame),
                recognition_id: None,
                action_id: None,
            })
            .expect("capture complete");
        assert!(matches!(
            capture_events.as_slice(),
            [PerformanceSemanticEvent::StutterDetected(_)]
        ));

        for observation in [
            PipelineEventObservation {
                event_type: EventType::RecognitionRequested,
                instance_id: "fixture-instance".to_owned(),
                observed_at_unix_ms: 2_600,
                frame_id: Some(second_frame),
                recognition_id: Some(recognition_id),
                action_id: None,
            },
            PipelineEventObservation {
                event_type: EventType::RecognitionCompleted,
                instance_id: "fixture-instance".to_owned(),
                observed_at_unix_ms: 3_000,
                frame_id: Some(second_frame),
                recognition_id: Some(recognition_id),
                action_id: None,
            },
            PipelineEventObservation {
                event_type: EventType::TaskEffectIntent,
                instance_id: "fixture-instance".to_owned(),
                observed_at_unix_ms: 3_100,
                frame_id: Some(second_frame),
                recognition_id: None,
                action_id: Some(action_id),
            },
            PipelineEventObservation {
                event_type: EventType::TaskEffectCompleted,
                instance_id: "fixture-instance".to_owned(),
                observed_at_unix_ms: 3_300,
                frame_id: Some(second_frame),
                recognition_id: None,
                action_id: Some(action_id),
            },
        ] {
            assert!(
                monitor
                    .observe_pipeline_event(observation)
                    .expect("pipeline event")
                    .is_empty()
            );
        }

        let context = monitor
            .context("fixture-instance", 3_300)
            .expect("pipeline context");
        assert_eq!(context.health, PerformanceMonitorHealth::Unavailable);
        assert_eq!(context.max_frame_gap_ms, Some(1_400));
        assert_eq!(context.max_capture_latency_ms, Some(1_300));
        assert_eq!(context.max_recognition_latency_ms, Some(400));
        assert_eq!(context.max_action_effect_latency_ms, Some(200));
    }

    #[test]
    fn raw_buffers_never_exceed_declared_bounds() {
        let config = PerformanceMonitorConfig {
            max_system_samples: 3,
            max_pipeline_samples: 2,
            ..PerformanceMonitorConfig::default()
        };
        let samples = (1..=6)
            .map(|index| Ok(sample(index * 1_000, 1_000)))
            .collect::<Vec<_>>();
        let mut monitor = PerformanceMonitor::enabled(
            config,
            Ok(Box::new(SequenceSampler {
                samples: samples.into(),
            })),
        )
        .expect("monitor");
        for index in 1..=6 {
            monitor.tick(index * 1_000).expect("tick");
        }
        for index in 1..=4 {
            monitor
                .record_pipeline_signal(
                    PipelinePerformanceSignal::new("fixture-instance", index * 1_000, 100)
                        .expect("signal"),
                )
                .expect("record");
        }
        assert_eq!(monitor.system_samples.len(), 3);
        assert_eq!(monitor.pipeline_samples.len(), 2);
    }

    #[test]
    fn out_of_order_pipeline_signal_fails_without_mutating_the_ring() {
        let mut monitor = monitor(Vec::new());
        monitor
            .record_pipeline_signal(
                PipelinePerformanceSignal::new("fixture-instance", 2_000, 100).expect("signal"),
            )
            .expect("first signal");
        let error = monitor
            .record_pipeline_signal(
                PipelinePerformanceSignal::new("fixture-instance", 1_000, 100).expect("signal"),
            )
            .expect_err("out-of-order signal");
        assert_eq!(error.code(), "performance_pipeline_time_invalid");
        assert_eq!(monitor.pipeline_samples.len(), 1);
    }

    #[test]
    fn pipeline_monitor_failure_is_explicitly_degraded_and_related_to_the_trigger() {
        let mut monitor = monitor(Vec::new());
        let identifiers = IdentifierIssuer::new().expect("identifiers");
        let frame_id = *identifiers.mint_frame_id().expect("frame").transport();
        let event_id = *identifiers.mint_event_id().expect("event").transport();
        let error = monitor
            .observe_pipeline_event(PipelineEventObservation {
                event_type: EventType::CaptureCompleted,
                instance_id: "fixture-instance".to_owned(),
                observed_at_unix_ms: 2_000,
                frame_id: Some(frame_id),
                recognition_id: None,
                action_id: None,
            })
            .expect_err("missing capture start");
        let degraded = monitor
            .record_monitor_failure(2_000, error.code(), Some(event_id))
            .expect("degraded monitor");
        assert!(matches!(
            degraded.events.as_slice(),
            [PerformanceSemanticEvent::MonitorDegraded(data)]
                if data.failure_code.as_deref() == Some("performance_pipeline_start_missing")
                    && data.unavailable_metrics == pipeline_metrics()
        ));
        let context = monitor
            .context("fixture-instance", 2_000)
            .expect("degraded context");
        assert_eq!(context.health, PerformanceMonitorHealth::Unavailable);
        assert_eq!(context.related_event_ids, vec![event_id]);
        assert!(!context.pressure_observed());
    }

    #[test]
    fn pipeline_monitor_recovers_only_after_a_successful_observation() {
        let mut monitor = monitor(vec![Ok(sample(1_000, 2_000))]);
        monitor.tick(1_000).expect("host sample");
        let degraded = monitor
            .record_monitor_failure(2_000, "performance_pipeline_start_missing", None)
            .expect("degraded monitor");
        assert!(matches!(
            degraded.events.as_slice(),
            [PerformanceSemanticEvent::MonitorDegraded(_)]
        ));
        let recovered = monitor
            .record_pipeline_success(2_100)
            .expect("recovered monitor");
        assert!(matches!(
            recovered.as_slice(),
            [PerformanceSemanticEvent::MonitorRecovered(data)]
                if data.health == PerformanceMonitorHealth::Partial
        ));
    }

    #[test]
    fn repeated_sampler_failure_degrades_then_stops_bounded_sampling() {
        let mut monitor = monitor(vec![
            Err("performance_counter_failed"),
            Err("performance_counter_failed"),
            Err("performance_counter_failed"),
        ]);
        let first = monitor.tick(1_000).expect("first");
        let second = monitor.tick(3_000).expect("second");
        let third = monitor.tick(5_000).expect("third");
        assert!(matches!(
            first.events.as_slice(),
            [PerformanceSemanticEvent::MonitorDegraded(_)]
        ));
        assert_eq!(first.events[0].severity(), EventSeverity::Warning);
        assert!(second.events.is_empty());
        assert!(third.stop_sampling);
        assert!(matches!(
            third.events.as_slice(),
            [PerformanceSemanticEvent::MonitorDegraded(data)] if data.terminal
        ));
        assert_eq!(third.events[0].severity(), EventSeverity::Error);
        let stopped = monitor.tick(7_000).expect("stopped monitor");
        assert!(stopped.stop_sampling);
        assert!(stopped.events.is_empty());
        monitor
            .record_pipeline_signal(
                PipelinePerformanceSignal::new("fixture-instance", 7_000, 1_500)
                    .expect("pipeline signal"),
            )
            .expect("pipeline collection remains available");
        let context = monitor.context("fixture-instance", 5_000).expect("context");
        assert_eq!(context.health, PerformanceMonitorHealth::Unavailable);
        assert!(!context.pressure_observed());
        let pipeline_context = monitor
            .context("fixture-instance", 7_000)
            .expect("pipeline context");
        assert_eq!(pipeline_context.max_frame_gap_ms, Some(1_500));
    }
}
