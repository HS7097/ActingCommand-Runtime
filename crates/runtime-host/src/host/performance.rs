// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use crate::events::is_sanitization_failure;
use crate::performance::rejected_event_code;
use crate::planning::collect_maintenance_evidence;
use actingcommand_policy::assess_predictive_maintenance;

pub(super) enum CapacityUse {
    Business,
    Drain,
}

/// One observation event's append: recorded, or its draft rejected by contract sanitization.
enum ObservationAppend {
    Recorded,
    Rejected(RuntimeHostError),
}

impl HostShared {
    pub(super) fn assess_and_publish_predictive_maintenance(
        &self,
        query: &MaintenanceLedgerQuery,
    ) -> RuntimeHostResult<MaintenanceAssessment> {
        let result: RuntimeHostResult<MaintenanceAssessment> = (|| {
            let evidence = collect_maintenance_evidence(&self.ledger, query)?;
            let assessment = assess_predictive_maintenance(&evidence, query.trend_policy())
                .map_err(|error| {
                    RuntimeHostError::request(
                        error.code(),
                        "assess_predictive_maintenance",
                        RuntimeErrorCode::InvalidRequest,
                    )
                })?;
            if assessment.recheck_suggested() {
                let observed_at_unix_ms = evidence
                    .durations
                    .iter()
                    .map(|sample| sample.observed_at_unix_ms)
                    .chain(
                        evidence
                            .confidences
                            .iter()
                            .map(|sample| sample.observed_at_unix_ms),
                    )
                    .max()
                    .ok_or_else(|| {
                        RuntimeHostError::fatal(
                            "maintenance_evidence_timestamp_missing",
                            "assess_predictive_maintenance",
                            RuntimeErrorCode::RuntimeFatal,
                        )
                    })?;
                self.record_policy_planning_signal(PolicyPlanningSignalEventData {
                    signal_id: format!("signal:{}", assessment.assessment_id),
                    instance_id: query.instance_id().to_owned(),
                    task_id: Some(query.task_id().to_owned()),
                    kind: actingcommand_contract::PolicyPlanningSignalKind::DriftPredicted,
                    fact_code: "maintenance_recheck_suggested".to_owned(),
                    observed_at_unix_ms,
                    detection_budget: None,
                })?;
            }
            Ok(assessment)
        })();
        if let Err(error) = &result
            && error.is_fatal()
        {
            self.fatal.mark(error.clone())?;
        }
        result
    }

    fn sample_performance(&self, observed_at_unix_ms: u64) -> RuntimeHostResult<bool> {
        // Drain-only confirmation of deferred appends on the tick that hosts the summary
        // producer; a failed reply ends the monitor through its existing fatal path.
        self.confirm_deferred_appends(Duration::ZERO, "confirm_deferred_appends")?;
        let (tick, control_observation) = {
            let mut performance = lock(&self.performance, "sample_performance")?;
            let mut tick = if performance.counters_enabled() {
                performance.tick(observed_at_unix_ms)?
            } else {
                PerformanceTick {
                    events: Vec::new(),
                    stop_sampling: true,
                }
            };
            // The one summary producer runs after this tick's system sample is ingested.
            performance.sample_and_record_capacity(&self.ledger, &self.events)?;
            tick.stop_sampling &= !performance.capacity_enabled();
            performance.attach_ledger_sample(&mut tick, &self.ledger)?;
            let observation = if performance.counters_enabled() {
                performance.control_observation(observed_at_unix_ms)?
            } else {
                None
            };
            (tick, observation)
        };
        let PerformanceTick {
            events,
            stop_sampling,
        } = tick;
        self.record_performance_events(&events)?;
        if let Some(observation) = control_observation {
            self.reconcile_performance_control(observation)?;
        }
        Ok(stop_sampling)
    }

    /// Called after replay resolution and immediately before authorizing new business.
    pub(super) fn admit_capacity(
        &self,
    ) -> RuntimeHostResult<actingcommand_contract::CapacityDecision> {
        lock(&self.performance, "admit_capacity")?.admit_capacity()
    }

    pub(super) fn require_business_capacity(
        &self,
        links: EventLinksDraft,
    ) -> Result<(), RequestFailure> {
        if let Err(error) = self.admit_capacity() {
            if error.is_fatal() {
                return Err(RequestFailure::poison_without_terminal(error));
            }
            let event = self
                .append_event_raw(
                    EventSeverity::Warning,
                    EventSource::Scheduler,
                    OriginModule::Scheduler,
                    EventActor::Scheduler,
                    links.clone(),
                    SchedulerPayloadDraft::denied(
                        EventAction::ScheduleAdmit,
                        DiagnosticCode::RuntimeDiagnostic,
                        AuditInput::new(),
                    ),
                )
                .map_err(RequestFailure::poison_without_terminal)?;
            self.record_required_failure(&error, &event, links)
                .map_err(RequestFailure::poison_without_terminal)?;
            return Err(RequestFailure::request(
                error,
                RuntimeReceiptState::Denied,
                Some(terminal(&event)),
            ));
        }
        Ok(())
    }

    /// Select a successor only on fresh capacity; refusing it does not refuse the old owner's drain.
    pub(super) fn capacity_allows_transfer(&self, token: &LeaseToken) -> RuntimeHostResult<bool> {
        let Err(error) = self.admit_capacity() else {
            return Ok(true);
        };
        if error.is_fatal() {
            return Err(error);
        }
        self.append_lifecycle_failure(
            RuntimeLifecycleFailureStage::OperationCleanup,
            RuntimeLifecycleFailure::Host(&error),
            self.events
                .synthetic_links(token, self.events.action_id()?)?,
            None,
        )?;
        Ok(false)
    }

    pub(super) fn reconcile_performance_control(
        &self,
        observation: crate::PerformanceControlObservation,
    ) -> RuntimeHostResult<()> {
        let workloads =
            lock(&self.policy, "read_performance_workloads")?.active_performance_workloads()?;
        let control_events = lock(&self.performance_control, "reconcile_performance_control")?
            .observe(observation, &workloads)?
            .into_iter()
            .map(PerformanceSemanticEvent::BalanceChanged)
            .collect::<Vec<_>>();
        self.record_performance_events(&control_events)
    }

    pub(super) fn record_pipeline_performance(
        &self,
        signal: PipelinePerformanceSignal,
    ) -> RuntimeHostResult<()> {
        let events = lock(&self.performance, "record_pipeline_performance")?
            .record_pipeline_signal(signal)?;
        self.record_performance_events(&events)
    }

    pub(super) fn performance_context(
        &self,
        instance_id: &str,
        observed_at_unix_ms: u64,
    ) -> RuntimeHostResult<PerformanceContext> {
        lock(&self.performance, "read_performance_context")?
            .context(instance_id, observed_at_unix_ms)
    }

    pub(super) fn performance_control_directive(
        &self,
        instance_id: &str,
    ) -> RuntimeHostResult<PerformanceControlDirective> {
        lock(
            &self.performance_control,
            "read_performance_control_directive",
        )?
        .directive(instance_id)
    }

    /// An observation event the contract rejects is dropped and degrades the monitor
    /// instead of ending the runtime; every other failure stays fatal.
    pub(super) fn record_performance_events(
        &self,
        events: &[PerformanceSemanticEvent],
    ) -> RuntimeHostResult<()> {
        for event in events {
            if let ObservationAppend::Rejected(error) = self.record_performance_event(event)? {
                self.degrade_performance_monitor(event.event_type(), error)?;
            }
        }
        Ok(())
    }

    fn record_performance_event(
        &self,
        event: &PerformanceSemanticEvent,
    ) -> RuntimeHostResult<ObservationAppend> {
        let (result, observation) = self.append_event_observed(
            event.severity(),
            EventSource::Runtime,
            OriginModule::PerformanceMonitor,
            EventActor::Runtime,
            self.events.system_links()?,
            event.payload(),
        );
        let persisted = match result {
            Ok(persisted) => persisted,
            Err(failure) => {
                let error = *failure.error;
                // The append hook also sanitizes fact invalidations once this draft is
                // persisted; only the draft stage itself rejecting the event is the
                // contract's verdict on the observation.
                if observation.draft.result == Some(TaskTimingResult::Err)
                    && is_sanitization_failure(&error)
                {
                    return Ok(ObservationAppend::Rejected(error));
                }
                return Err(error);
            }
        };
        let mut performance = lock(&self.performance, "record_performance_event_reference")?;
        if !matches!(event, PerformanceSemanticEvent::BalanceChanged(_))
            || performance.counters_enabled()
        {
            performance.record_event_reference(event, *persisted.event_id())?;
        }
        Ok(ObservationAppend::Recorded)
    }

    /// The dropped event is visible as `perf.monitor_degraded` carrying the contract's
    /// sanitization code and the rejected event type; a degraded event that fails itself is
    /// fatal as before.
    fn degrade_performance_monitor(
        &self,
        rejected: EventType,
        rejection: RuntimeHostError,
    ) -> RuntimeHostResult<()> {
        let tick = {
            let mut performance = lock(&self.performance, "degrade_performance_monitor")?;
            if !performance.counters_enabled() {
                return Err(rejection);
            }
            let code = rejected_event_code(rejection.code(), rejected)?;
            performance.record_monitor_failure(unix_ms_now()?, &code, None)?
        };
        for event in &tick.events {
            if let ObservationAppend::Rejected(error) = self.record_performance_event(event)? {
                return Err(error);
            }
        }
        Ok(())
    }

    pub(super) fn observe_pipeline_event(&self, event: &PersistedEvent) -> RuntimeHostResult<()> {
        if !is_pipeline_event(event.event_type())
            || !lock(&self.performance, "read_performance_monitor_state")?.accepts_pipeline_events()
        {
            return Ok(());
        }
        let result: RuntimeHostResult<Vec<PerformanceSemanticEvent>> = (|| {
            let instance_id = event.links().instance_id().ok_or_else(|| {
                RuntimeHostError::fatal(
                    "performance_pipeline_instance_missing",
                    "observe_performance_pipeline_event",
                    RuntimeErrorCode::RuntimeFatal,
                )
            })?;
            let instance_alias = lock(
                &self.registered_instances,
                "resolve_performance_pipeline_instance",
            )?
            .get(instance_id)
            .map(|instance| instance.instance_alias.clone())
            .ok_or_else(|| {
                RuntimeHostError::fatal(
                    "performance_pipeline_instance_unknown",
                    "observe_performance_pipeline_event",
                    RuntimeErrorCode::RuntimeFatal,
                )
            })?;
            let (touch_response_us, capture_acquire_us) = match event.payload() {
                EventPayload::Input(InputPayload::Committed(payload)) => {
                    (payload.touch_response_us(), None)
                }
                EventPayload::Capture(CapturePayload::Completed(payload)) => {
                    (None, payload.capture_acquire_us())
                }
                _ => (None, None),
            };
            let observation = PipelineEventObservation {
                event_type: event.event_type(),
                instance_id: instance_alias,
                observed_at_unix_ms: event.timestamp_unix_ms(),
                frame_id: event.links().frame_id().copied(),
                recognition_id: event.links().recognition_id().copied(),
                action_id: event.links().action_id().copied(),
                touch_response_us,
                capture_acquire_us,
            };
            lock(&self.performance, "observe_performance_pipeline_event")?
                .observe_pipeline_event(observation)
        })();
        let semantic_events = match result {
            Ok(mut events) => {
                let mut recovered =
                    lock(&self.performance, "recover_performance_pipeline_monitor")?
                        .record_pipeline_success(event.timestamp_unix_ms())?;
                events.append(&mut recovered);
                events
            }
            Err(error) => {
                let tick = lock(&self.performance, "degrade_performance_pipeline_monitor")?
                    .record_monitor_failure(
                        event.timestamp_unix_ms(),
                        error.code(),
                        Some(*event.event_id()),
                    )?;
                tick.events
            }
        };
        self.record_performance_events(&semantic_events)
    }
}

const fn is_pipeline_event(event_type: EventType) -> bool {
    matches!(
        event_type,
        EventType::InputCommitted
            | EventType::CaptureRequested
            | EventType::CaptureCompleted
            | EventType::CaptureFailed
            | EventType::RecognitionRequested
            | EventType::RecognitionCompleted
            | EventType::RecognitionFailed
            | EventType::TaskEffectIntent
            | EventType::TaskEffectCompleted
            | EventType::TaskStepFinished
            | EventType::TaskCompleted
            | EventType::TaskFailed
            | EventType::TaskCancelled
    )
}

pub(super) fn performance_monitor_loop(
    shared: Arc<HostShared>,
    sample_interval: Duration,
) -> RuntimeHostResult<()> {
    let snapshot_interval = Duration::from_millis(RUNTIME_FACT_SNAPSHOT_INTERVAL_MS);
    let mut since_runtime_fact_snapshot = Duration::ZERO;
    while !shared.fatal.is_shutdown_requested() {
        thread::sleep(sample_interval);
        if shared.fatal.is_shutdown_requested() {
            break;
        }
        let Some(_work) = shared.begin_work()? else {
            break;
        };
        let observed_at_unix_ms = unix_ms_now()?;
        let stop_sampling = match shared.sample_performance(observed_at_unix_ms) {
            Ok(stop_sampling) => stop_sampling,
            Err(error) => {
                shared.fatal.mark(error.clone())?;
                return Err(error);
            }
        };
        let retention_enabled = shared.maintain_frame_retention()?;
        // Sealed at most once per RUNTIME_FACT_SNAPSHOT_INTERVAL_MS, whatever the sample interval.
        since_runtime_fact_snapshot = since_runtime_fact_snapshot.saturating_add(sample_interval);
        if since_runtime_fact_snapshot >= snapshot_interval {
            since_runtime_fact_snapshot = Duration::ZERO;
            if let Err(error) = shared.append_runtime_fact_snapshot_if_dirty() {
                shared.fatal.mark(error.clone())?;
                return Err(error);
            }
        }
        if stop_sampling && !retention_enabled {
            break;
        }
    }
    Ok(())
}
