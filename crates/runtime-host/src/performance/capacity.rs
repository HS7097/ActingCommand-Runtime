// SPDX-License-Identifier: AGPL-3.0-only

use super::{PerformanceMonitor, PerformanceMonitorConfig, system_performance_sampler};
use crate::events::RuntimeEvents;
use crate::{RuntimeClock, RuntimeClockSample, RuntimeHostError, RuntimeHostResult};
use actingcommand_artifact_store::{
    ArtifactCapacityAdmission, ArtifactStore, ArtifactStoreError, ArtifactStoreResult,
};
use actingcommand_contract::{
    AuditInput, CapacityAdmissionOutcome, CapacityAdmissionReason, CapacityDecision,
    CapacityFactReference, CapacityNativeCause, CapacityPurpose, CapacityState, CapacityThresholds,
    CapacityVolumeSample, EventActor, EventSeverity, EventSource, LifecycleNativeDetail,
    OriginModule, OwnerEpoch, PerformanceCapacitySample, PerformanceContext, PerformanceMetric,
    PerformanceMonitorHealth, PerformanceMonitorStateEventData, PerformancePayloadDraft,
    PerformancePressureEventData, PerformancePressureKind, PerformancePressureRecord,
    PerformancePressureSeverity, PerformancePressureValue, PerformanceSummaryEventData,
    RuntimeErrorCode,
};
use actingcommand_host_metrics::{CapacityTarget, capacity_volume, sample_capacity};
use actingcommand_ledger::{GlobalLedger, PersistedEvent};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub(crate) struct CapacityPreflightConfig {
    pub(crate) performance: Option<PerformanceMonitorConfig>,
    pub(crate) thresholds: CapacityThresholds,
}

pub(crate) struct CapacityRoots {
    owner_epoch: OwnerEpoch,
    targets: Vec<(CapacityPurpose, CapacityTarget)>,
}

impl CapacityRoots {
    pub(crate) fn new(
        owner_epoch: OwnerEpoch,
        state: &Path,
        artifacts: &Path,
    ) -> RuntimeHostResult<Self> {
        let exe = std::env::current_exe()
            .map_err(|error| native_failure("capacity_installation_path_failed", error))?;
        let installation = exe
            .parent()
            .ok_or_else(|| failure("capacity_installation_path_failed"))?;
        Ok(Self {
            owner_epoch,
            targets: vec![
                (
                    CapacityPurpose::Installation,
                    CapacityTarget {
                        path: installation.to_path_buf(),
                    },
                ),
                (
                    CapacityPurpose::State,
                    CapacityTarget {
                        path: state.to_path_buf(),
                    },
                ),
                (
                    CapacityPurpose::Artifact,
                    CapacityTarget {
                        path: artifacts.join("artifacts"),
                    },
                ),
                (
                    CapacityPurpose::ArtifactStaging,
                    CapacityTarget {
                        path: artifacts.to_path_buf(),
                    },
                ),
            ],
        })
    }
}

#[derive(Clone)]
struct CommittedCapacity {
    sample: PerformanceCapacitySample,
    reference: CapacityFactReference,
}

/// A projection of one B3 commit, never an independent sampler or source of facts.
struct CapacityProjection {
    owner_epoch: OwnerEpoch,
    targets: Vec<(CapacityPurpose, CapacityTarget)>,
    clock: Arc<dyn RuntimeClock>,
    committed: Mutex<Option<CommittedCapacity>>,
}

impl CapacityProjection {
    fn decide(&self, target: Option<&Path>, bytes: u64) -> RuntimeHostResult<CapacityDecision> {
        let committed = self
            .committed
            .lock()
            .map_err(|_| failure("capacity_projection_poisoned"))?
            .clone();
        let bindings = self
            .targets
            .iter()
            .map(|(purpose, target)| (*purpose, capacity_volume(&target.path)))
            .collect::<Vec<_>>();
        let target_volume = target.map(capacity_volume);
        // Time spent resolving a binding cannot extend the sample's freshness.
        let now = self.clock.sample()?;
        let mut decision = CapacityDecision {
            owner_epoch: self.owner_epoch,
            decided_at_unix_ms: now.unix_ms,
            decided_at_monotonic_ms: now.monotonic_ms,
            requested_bytes: bytes,
            target_volume: target_volume
                .as_ref()
                .and_then(|binding| binding.as_ref().ok())
                .cloned(),
            outcome: CapacityAdmissionOutcome::Unknown,
            reason: CapacityAdmissionReason::NoCommittedFact,
            fact: committed.as_ref().map(|value| value.reference.clone()),
        };
        let Some(committed) = committed else {
            return Ok(decision);
        };
        let sample = &committed.sample;
        if sample.owner_epoch != self.owner_epoch {
            decision.reason = CapacityAdmissionReason::OwnerChanged;
            return Ok(decision);
        }
        if sample
            .volumes
            .iter()
            .any(|volume| volume.available_bytes.is_none())
        {
            decision.reason = CapacityAdmissionReason::SampleUnavailable;
            return Ok(decision);
        }
        if !now
            .unix_ms
            .checked_sub(sample.observed_at_unix_ms)
            .is_some_and(|age| age <= sample.freshness_ms)
            || !now
                .monotonic_ms
                .checked_sub(sample.observed_at_monotonic_ms)
                .is_some_and(|age| age <= sample.freshness_ms)
        {
            decision.reason = CapacityAdmissionReason::OutsideFreshness;
            return Ok(decision);
        }
        if bindings.iter().any(|(purpose, binding)| {
            !sample.volumes.iter().any(|volume| {
                volume.purposes.contains(purpose)
                    && binding.as_ref().ok() == volume.volume_id.as_ref()
                    && binding.is_ok()
            })
        }) || target_volume.as_ref().is_some_and(|binding| {
            binding.as_ref().ok().is_none_or(|id| {
                !sample.volumes.iter().any(|volume| {
                    volume.volume_id.as_ref() == Some(id)
                        && volume.purposes.iter().any(|purpose| {
                            matches!(
                                purpose,
                                CapacityPurpose::Artifact | CapacityPurpose::ArtifactStaging
                            )
                        })
                })
            })
        }) {
            decision.reason = CapacityAdmissionReason::BindingChanged;
            return Ok(decision);
        }
        let mut outcome = CapacityAdmissionOutcome::Allowed;
        for volume in &sample.volumes {
            let Some(free) = volume.available_bytes else {
                decision.reason = CapacityAdmissionReason::SampleUnavailable;
                return Ok(decision);
            };
            let bytes = if target_volume
                .as_ref()
                .and_then(|binding| binding.as_ref().ok())
                == volume.volume_id.as_ref()
            {
                bytes
            } else {
                0
            };
            let Some(required) = sample.thresholds.hard_bytes.checked_add(bytes) else {
                decision.outcome = CapacityAdmissionOutcome::RequiredBytesOverflow;
                decision.reason = CapacityAdmissionReason::KnownBytesOverflow;
                return Ok(decision);
            };
            if free < required {
                decision.outcome = CapacityAdmissionOutcome::HardPressure;
                decision.reason = CapacityAdmissionReason::HardThreshold;
                return Ok(decision);
            }
            if free < sample.thresholds.soft_bytes {
                outcome = CapacityAdmissionOutcome::SoftPressure;
            }
        }
        decision.outcome = outcome;
        decision.reason = CapacityAdmissionReason::FreshSample;
        Ok(decision)
    }

    fn commit(
        &self,
        sample: PerformanceCapacitySample,
        event: &PersistedEvent,
    ) -> RuntimeHostResult<()> {
        let reference = CapacityFactReference {
            event_id: *event.event_id(),
            sequence: event.sequence(),
            owner_epoch: sample.owner_epoch,
            observed_at_unix_ms: sample.observed_at_unix_ms,
            observed_at_monotonic_ms: sample.observed_at_monotonic_ms,
        };
        *self
            .committed
            .lock()
            .map_err(|_| failure("capacity_projection_poisoned"))? =
            Some(CommittedCapacity { sample, reference });
        Ok(())
    }
}

impl ArtifactCapacityAdmission for CapacityProjection {
    fn decide(&self, path: &Path, bytes: u64) -> ArtifactStoreResult<CapacityDecision> {
        self.decide(Some(path), bytes).map_err(|error| {
            ArtifactStoreError::fatal(error.code(), error.operation(), error.to_string())
        })
    }
}

pub(super) struct CapacityMonitor {
    pub(super) interval: Duration,
    thresholds: CapacityThresholds,
    projection: Arc<CapacityProjection>,
    consecutive_failures: u16,
    pressure: Option<PerformancePressureRecord>,
}

impl PerformanceMonitor {
    /// S3 calls this after Ledger opens and before Provider/business construction.
    pub(crate) fn preflight_capacity(
        config: CapacityPreflightConfig,
        roots: CapacityRoots,
        ledger: &GlobalLedger,
        events: &RuntimeEvents,
        artifacts: &ArtifactStore,
        clock: Arc<dyn RuntimeClock>,
    ) -> RuntimeHostResult<Self> {
        config
            .thresholds
            .validate()
            .map_err(|_| failure("invalid_capacity_thresholds"))?;
        let interval = config
            .performance
            .as_ref()
            .map_or(Duration::from_secs(2), |config| config.sample_interval);
        let mut monitor = match config.performance {
            Some(config) => Self::enabled(config, system_performance_sampler())?,
            None => Self::disabled(),
        };
        let projection = Arc::new(CapacityProjection {
            owner_epoch: roots.owner_epoch,
            targets: roots.targets,
            clock,
            committed: Mutex::new(None),
        });
        monitor.capacity = Some(CapacityMonitor {
            interval,
            thresholds: config.thresholds,
            projection: Arc::clone(&projection),
            consecutive_failures: 0,
            pressure: None,
        });
        monitor.sample_and_record_capacity(ledger, events)?;
        artifacts
            .install_capacity_admission(projection)
            .map_err(RuntimeHostError::artifact)?;
        monitor.admit_capacity()?;
        Ok(monitor)
    }

    pub(crate) fn capacity_enabled(&self) -> bool {
        self.capacity.is_some()
    }

    pub(crate) fn counters_enabled(&self) -> bool {
        self.config.is_some()
    }

    pub(crate) fn admit_capacity(&self) -> RuntimeHostResult<CapacityDecision> {
        let capacity = self
            .capacity
            .as_ref()
            .ok_or_else(|| failure("capacity_owner_missing"))?;
        let decision = capacity.projection.decide(None, 0)?;
        if !decision.outcome.allows() {
            let mut error = RuntimeHostError::request(
                "capacity_admission_refused",
                "admit_new_business",
                RuntimeErrorCode::InvalidRequest,
            );
            error.lifecycle.capacity = Some(decision);
            return Err(error);
        }
        Ok(decision)
    }

    pub(crate) fn sample_and_record_capacity(
        &mut self,
        ledger: &GlobalLedger,
        events: &RuntimeEvents,
    ) -> RuntimeHostResult<()> {
        let Some(capacity) = self.capacity.as_mut() else {
            return Ok(());
        };
        let now = capacity.projection.clock.sample()?;
        let targets = capacity
            .projection
            .targets
            .iter()
            .map(|(_, target)| target.clone())
            .collect::<Vec<_>>();
        let samples = match self.sampler.as_mut() {
            Some(sampler) => sampler.sample_capacity(&targets),
            None => sample_capacity(&targets),
        };
        let volumes = samples
            .into_iter()
            .map(|sample| {
                let (available_bytes, state, cause) = match sample.available_bytes {
                    Ok(bytes) => (
                        Some(bytes),
                        if bytes < capacity.thresholds.hard_bytes {
                            CapacityState::HardPressure
                        } else if bytes < capacity.thresholds.soft_bytes {
                            CapacityState::SoftPressure
                        } else {
                            CapacityState::Sufficient
                        },
                        None,
                    ),
                    Err(error) => (
                        None,
                        CapacityState::Unknown,
                        Some(CapacityNativeCause {
                            operation: error.operation.into(),
                            raw_os_error: error.raw_os_error,
                            detail: bounded_native(&error.detail),
                        }),
                    ),
                };
                let purposes = sample
                    .targets
                    .into_iter()
                    .map(|index| {
                        capacity
                            .projection
                            .targets
                            .get(index)
                            .map(|(purpose, _)| *purpose)
                            .ok_or_else(|| failure("invalid_capacity_sample_target"))
                    })
                    .collect::<RuntimeHostResult<Vec<_>>>()?;
                Ok(CapacityVolumeSample {
                    volume_id: sample.volume_id,
                    purposes,
                    available_bytes,
                    state,
                    cause,
                })
            })
            .collect::<RuntimeHostResult<Vec<_>>>()?;
        let sample = PerformanceCapacitySample {
            owner_epoch: capacity.projection.owner_epoch,
            observed_at_unix_ms: now.unix_ms,
            observed_at_monotonic_ms: now.monotonic_ms,
            freshness_ms: capacity.interval.as_millis() as u64 * 2,
            thresholds: capacity.thresholds,
            volumes,
        };
        sample
            .validate()
            .map_err(|_| failure("invalid_capacity_sample"))?;
        let unknown = sample
            .volumes
            .iter()
            .any(|volume| volume.state == CapacityState::Unknown);
        let pressure = sample.volumes.iter().any(|volume| {
            matches!(
                volume.state,
                CapacityState::SoftPressure | CapacityState::HardPressure
            )
        });
        let severity = if unknown || pressure {
            EventSeverity::Warning
        } else {
            EventSeverity::Info
        };
        let summary = PerformanceSummaryEventData {
            context: PerformanceContext::unavailable(now.unix_ms),
            foreground: None,
            owned_processes: Vec::new(),
            third_party_high_load: Vec::new(),
            ledger_commits: None,
            capacity: Some(sample.clone()),
        };
        let event = append(
            ledger,
            events,
            severity,
            PerformancePayloadDraft::summary(summary.clone(), AuditInput::new()),
        );
        let event = match event {
            Ok(event) => event,
            Err(error) => {
                *capacity
                    .projection
                    .committed
                    .lock()
                    .map_err(|_| failure("capacity_projection_poisoned"))? = None;
                return Err(error);
            }
        };
        capacity.projection.commit(sample.clone(), &event)?;
        capacity.record_transitions(&sample, now, ledger, events)?;
        if self.config.is_some() {
            self.record_event_reference(
                &super::PerformanceSemanticEvent::Summary(Box::new(summary)),
                *event.event_id(),
            )?;
        }
        Ok(())
    }
}

impl CapacityMonitor {
    fn record_transitions(
        &mut self,
        sample: &PerformanceCapacitySample,
        now: RuntimeClockSample,
        ledger: &GlobalLedger,
        events: &RuntimeEvents,
    ) -> RuntimeHostResult<()> {
        let unknown = sample
            .volumes
            .iter()
            .any(|volume| volume.state == CapacityState::Unknown);
        if unknown {
            self.consecutive_failures = self.consecutive_failures.saturating_add(1);
            if matches!(self.consecutive_failures, 1 | 3) {
                append(
                    ledger,
                    events,
                    if self.consecutive_failures == 3 {
                        EventSeverity::Error
                    } else {
                        EventSeverity::Warning
                    },
                    PerformancePayloadDraft::monitor_degraded(
                        PerformanceMonitorStateEventData {
                            observed_at_unix_ms: now.unix_ms,
                            health: PerformanceMonitorHealth::Degraded,
                            failure_code: Some("capacity_sample_unavailable".into()),
                            consecutive_failures: self.consecutive_failures,
                            terminal: false,
                            unavailable_metrics: vec![PerformanceMetric::DiskCapacity],
                        },
                        AuditInput::new(),
                    ),
                )?;
            }
            return Ok(());
        }
        if self.consecutive_failures > 0 {
            append(
                ledger,
                events,
                EventSeverity::Info,
                PerformancePayloadDraft::monitor_recovered(
                    PerformanceMonitorStateEventData {
                        observed_at_unix_ms: now.unix_ms,
                        health: PerformanceMonitorHealth::Healthy,
                        failure_code: None,
                        consecutive_failures: 0,
                        terminal: false,
                        unavailable_metrics: Vec::new(),
                    },
                    AuditInput::new(),
                ),
            )?;
            self.consecutive_failures = 0;
        }
        let free = sample
            .volumes
            .iter()
            .filter_map(|volume| volume.available_bytes)
            .min()
            .ok_or_else(|| failure("invalid_capacity_sample"))?;
        if free < self.thresholds.soft_bytes {
            let severity = if free < self.thresholds.hard_bytes {
                PerformancePressureSeverity::High
            } else {
                PerformancePressureSeverity::Elevated
            };
            let changed = self
                .pressure
                .as_ref()
                .is_none_or(|pressure| pressure.severity != severity);
            let record = PerformancePressureRecord {
                kind: PerformancePressureKind::DiskCapacity,
                severity,
                started_at_unix_ms: self.pressure.as_ref().map_or(now.unix_ms, |pressure| {
                    pressure.started_at_unix_ms.min(now.unix_ms)
                }),
                last_observed_at_unix_ms: now.unix_ms,
                peak: PerformancePressureValue::DiskCapacity {
                    available_bytes: free,
                    thresholds: self.thresholds,
                },
            };
            if changed {
                append(
                    ledger,
                    events,
                    if free < self.thresholds.hard_bytes {
                        EventSeverity::Error
                    } else {
                        EventSeverity::Warning
                    },
                    PerformancePayloadDraft::pressure_started(
                        PerformancePressureEventData {
                            observed_at_unix_ms: now.unix_ms,
                            pressure: record.clone(),
                        },
                        AuditInput::new(),
                    ),
                )?;
            }
            self.pressure = Some(record);
        } else if let Some(mut record) = self.pressure.take() {
            record.started_at_unix_ms = record.started_at_unix_ms.min(now.unix_ms);
            record.last_observed_at_unix_ms = now.unix_ms;
            append(
                ledger,
                events,
                EventSeverity::Info,
                PerformancePayloadDraft::pressure_ended(
                    PerformancePressureEventData {
                        observed_at_unix_ms: now.unix_ms,
                        pressure: record,
                    },
                    AuditInput::new(),
                ),
            )?;
        }
        Ok(())
    }
}

fn append(
    ledger: &GlobalLedger,
    events: &RuntimeEvents,
    severity: EventSeverity,
    payload: PerformancePayloadDraft,
) -> RuntimeHostResult<PersistedEvent> {
    let draft = events.draft(
        severity,
        EventSource::Runtime,
        OriginModule::PerformanceMonitor,
        EventActor::Runtime,
        events.system_links()?,
        payload,
    )?;
    ledger.append(events.sanitize(draft)?).map_err(|error| {
        let mut result = RuntimeHostError::fatal(
            error.code(),
            error.operation(),
            RuntimeErrorCode::LedgerFailure,
        );
        if let Some(detail) = error.detail() {
            result.lifecycle.native_detail = Some(Box::new(bounded_native(detail)));
        }
        result
    })
}

fn failure(code: &'static str) -> RuntimeHostError {
    RuntimeHostError::fatal(code, "sample_capacity", RuntimeErrorCode::RuntimeFatal)
}

fn native_failure(code: &'static str, error: std::io::Error) -> RuntimeHostError {
    let mut result = failure(code);
    result.lifecycle.raw_os_error = error.raw_os_error();
    result.lifecycle.native_detail = Some(Box::new(bounded_native(&error.to_string())));
    result
}

fn bounded_native(detail: &str) -> LifecycleNativeDetail {
    let mut end = detail.len().min(1024);
    while !detail.is_char_boundary(end) {
        end -= 1;
    }
    LifecycleNativeDetail::new(&detail[..end], end < detail.len())
}
