// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

pub(super) enum CapacityUse {
    Business,
    Drain,
}

impl HostShared {
    fn sample_performance(&self, observed_at_unix_ms: u64) -> RuntimeHostResult<bool> {
        let (tick, control_observation) = {
            let mut performance = lock(&self.performance, "sample_performance")?;
            performance.sample_and_record_capacity(&self.ledger, &self.events)?;
            let mut tick = if performance.counters_enabled() {
                performance.tick(observed_at_unix_ms)?
            } else {
                PerformanceTick {
                    events: Vec::new(),
                    stop_sampling: true,
                }
            };
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

    pub(super) fn record_performance_events(
        &self,
        events: &[PerformanceSemanticEvent],
    ) -> RuntimeHostResult<()> {
        for event in events {
            let payload = match event {
                PerformanceSemanticEvent::PressureStarted(data) => {
                    PerformancePayloadDraft::pressure_started(data.clone(), AuditInput::new())
                }
                PerformanceSemanticEvent::PressureEnded(data) => {
                    PerformancePayloadDraft::pressure_ended(data.clone(), AuditInput::new())
                }
                PerformanceSemanticEvent::StutterDetected(data) => {
                    PerformancePayloadDraft::stutter_detected(data.clone(), AuditInput::new())
                }
                PerformanceSemanticEvent::Summary(data) => {
                    PerformancePayloadDraft::summary(data.as_ref().clone(), AuditInput::new())
                }
                PerformanceSemanticEvent::MonitorDegraded(data) => {
                    PerformancePayloadDraft::monitor_degraded(data.clone(), AuditInput::new())
                }
                PerformanceSemanticEvent::MonitorRecovered(data) => {
                    PerformancePayloadDraft::monitor_recovered(data.clone(), AuditInput::new())
                }
                PerformanceSemanticEvent::BalanceChanged(data) => {
                    PerformancePayloadDraft::balance_changed(data.clone(), AuditInput::new())
                }
            };
            let persisted = self.append_event_raw(
                event.severity(),
                EventSource::Runtime,
                OriginModule::PerformanceMonitor,
                EventActor::Runtime,
                self.events.system_links()?,
                payload,
            )?;
            let mut performance = lock(&self.performance, "record_performance_event_reference")?;
            if !matches!(event, PerformanceSemanticEvent::BalanceChanged(_))
                || performance.counters_enabled()
            {
                performance.record_event_reference(event, *persisted.event_id())?;
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
            let observation = PipelineEventObservation {
                event_type: event.event_type(),
                instance_id: instance_alias,
                observed_at_unix_ms: event.timestamp_unix_ms(),
                frame_id: event.links().frame_id().copied(),
                recognition_id: event.links().recognition_id().copied(),
                action_id: event.links().action_id().copied(),
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
        EventType::CaptureRequested
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
        if stop_sampling && !retention_enabled {
            break;
        }
    }
    Ok(())
}
