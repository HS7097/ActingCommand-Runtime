// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use crate::codes::{HostCode, HostLocation, PacingCode, ProbeKind};
use crate::events::is_sanitization_failure;
use crate::performance::{
    PacingBackendRole, PacingSample, PacingSampleSource, rejected_event_outcome,
};
use crate::planning::collect_maintenance_evidence;
use actingcommand_contract::codes::ContractLocation;
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
                        ContractLocation::AssessPredictiveMaintenance,
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
                            HostCode::MaintenanceEvidenceTimestampMissing,
                            ContractLocation::AssessPredictiveMaintenance,
                            RuntimeErrorCode::RuntimeHalted,
                        )
                    })?;
                self.record_policy_planning_signal(PolicyPlanningSignalEventData {
                    signal_id: format!("signal:{}", assessment.assessment_id),
                    instance_id: query.instance_id().to_owned(),
                    task_id: Some(query.task_id().to_owned()),
                    kind: actingcommand_contract::PolicyPlanningSignalKind::DriftPredicted,
                    outcome: actingcommand_contract::outcome::Outcome::new(
                        HostCode::MaintenanceRecheckSuggested,
                    ),
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
        self.confirm_deferred_appends(Duration::ZERO, HostLocation::ConfirmDeferredAppends)?;
        // Workflow #385 P-m: the section in effect and the registered instances, each read and
        // released before the monitor's lock.
        let measurement = self.performance_measurement()?;
        let instances = self.pacing_instances()?;
        let (tick, control_observation, pipeline_quiet) = {
            let mut performance = lock(&self.performance, HostLocation::SamplePerformance)?;
            let mut tick = if performance.counters_enabled() {
                // The shadow evaluates first, so this tick's summary carries its views; it acts
                // on nothing (coordinator ruling 7 of wave 13).
                let mut events =
                    performance.evaluate_pacing(observed_at_unix_ms, &measurement, &instances)?;
                let mut tick = performance.tick(observed_at_unix_ms)?;
                events.append(&mut tick.events);
                tick.events = events;
                tick
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
            let (observation, pipeline_quiet) = if performance.counters_enabled() {
                (
                    performance.control_observation(observed_at_unix_ms)?,
                    Some(performance.pipeline_quiet(observed_at_unix_ms)?),
                )
            } else {
                (None, None)
            };
            (tick, observation, pipeline_quiet)
        };
        let PerformanceTick {
            events,
            stop_sampling,
        } = tick;
        self.record_performance_events(&events)?;
        match (control_observation, pipeline_quiet) {
            (Some(observation), _) => self.reconcile_performance_control(observation)?,
            (None, Some(pipeline_quiet)) => {
                self.reconcile_performance_tick(observed_at_unix_ms, pipeline_quiet)?
            }
            // No enabled monitor: the controller has no evidence source.
            (None, None) => {}
        }
        Ok(stop_sampling)
    }

    /// Called after replay resolution and immediately before authorizing new business.
    pub(super) fn admit_capacity(
        &self,
    ) -> RuntimeHostResult<actingcommand_contract::CapacityDecision> {
        lock(&self.performance, HostLocation::AdmitCapacity)?.admit_capacity()
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
        let workloads = lock(self.policy()?, HostLocation::ReadPerformanceWorkloads)?
            .active_performance_workloads()?;
        let control_events = lock(
            &self.performance_control,
            HostLocation::ReconcilePerformanceControl,
        )?
        .observe(observation, &workloads)?
        .into_iter()
        .map(PerformanceSemanticEvent::BalanceChanged)
        .collect::<Vec<_>>();
        self.record_performance_events(&control_events)
    }

    /// A tick without a fresh sample (performance-control.md "Recovery without evidence").
    fn reconcile_performance_tick(
        &self,
        observed_at_unix_ms: u64,
        pipeline_quiet: bool,
    ) -> RuntimeHostResult<()> {
        // Only this monitor thread changes control levels (the one production `observe` and
        // `observe_without_evidence` caller), so `raised` read here still holds below. Each
        // lock below is taken in its own statement: holding scheduler while taking policy
        // would add an edge against the existing policy -> scheduler order.
        let raised = lock(
            &self.performance_control,
            HostLocation::ReadPerformanceControlRaised,
        )?
        .raised();
        let mut owned_work = !pipeline_quiet;
        if raised && !owned_work {
            owned_work = !lock(&self.scheduler, HostLocation::ReadPerformanceOwnedLeases)?
                .active_instance_ids()
                .is_empty();
        }
        if raised && !owned_work {
            owned_work = !lock(self.policy()?, HostLocation::ReadPerformanceWorkloads)?
                .active_performance_workloads()?
                .is_empty();
        }
        let control_events = lock(
            &self.performance_control,
            HostLocation::ReconcilePerformanceTick,
        )?
        .observe_without_evidence(observed_at_unix_ms, owned_work)?
        .into_iter()
        .map(PerformanceSemanticEvent::BalanceChanged)
        .collect::<Vec<_>>();
        self.record_performance_events(&control_events)
    }

    pub(super) fn record_pipeline_performance(
        &self,
        signal: PipelinePerformanceSignal,
    ) -> RuntimeHostResult<()> {
        let events = lock(&self.performance, HostLocation::RecordPipelinePerformance)?
            .record_pipeline_signal(signal)?;
        self.record_performance_events(&events)
    }

    pub(super) fn performance_context(
        &self,
        instance_id: &str,
        observed_at_unix_ms: u64,
    ) -> RuntimeHostResult<PerformanceContext> {
        lock(&self.performance, HostLocation::ReadPerformanceContext)?
            .context(instance_id, observed_at_unix_ms)
    }

    pub(super) fn performance_control_directive(
        &self,
        instance_id: &str,
    ) -> RuntimeHostResult<PerformanceControlDirective> {
        lock(
            &self.performance_control,
            HostLocation::ReadPerformanceControlDirective,
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
        let mut performance = lock(
            &self.performance,
            HostLocation::RecordPerformanceEventReference,
        )?;
        if !matches!(event, PerformanceSemanticEvent::BalanceChanged(_))
            || performance.counters_enabled()
        {
            performance.record_event_reference(event, *persisted.event_id())?;
        }
        Ok(ObservationAppend::Recorded)
    }

    /// The dropped event is visible as `perf.monitor_degraded` whose outcome is
    /// `performance_event_rejected` with the rejected event type, caused by the contract's
    /// sanitization outcome; a degraded event that fails itself is fatal as before.
    fn degrade_performance_monitor(
        &self,
        rejected: EventType,
        rejection: RuntimeHostError,
    ) -> RuntimeHostResult<()> {
        let tick = {
            let mut performance = lock(&self.performance, HostLocation::DegradePerformanceMonitor)?;
            if !performance.counters_enabled() {
                return Err(rejection);
            }
            let failure = rejected_event_outcome(&rejection, rejected);
            performance.record_monitor_failure(unix_ms_now()?, failure, None)?
        };
        for event in &tick.events {
            if let ObservationAppend::Rejected(error) = self.record_performance_event(event)? {
                return Err(error);
            }
        }
        Ok(())
    }

    pub(super) fn observe_pipeline_event(&self, event: &PersistedEvent) -> RuntimeHostResult<()> {
        // Workflow #385 P-m: a backend open names the backend an instance's baselines are kept
        // for; a frame's acquisition and an input's response are the shadow's samples, taken
        // whether or not the pipeline monitor still pairs events.
        if event.event_type() == EventType::RuntimeLifecycleObserved {
            return self.observe_pacing_backend(event);
        }
        self.record_pacing_sample_of(event)?;
        if !is_pipeline_event(event.event_type())
            || !lock(&self.performance, HostLocation::ReadPerformanceMonitorState)?
                .accepts_pipeline_events()
        {
            return Ok(());
        }
        let result: RuntimeHostResult<Vec<PerformanceSemanticEvent>> = (|| {
            let instance_id = event.links().instance_id().ok_or_else(|| {
                RuntimeHostError::fatal(
                    HostCode::PerformancePipelineInstanceMissing,
                    HostLocation::ObservePerformancePipelineEvent,
                    RuntimeErrorCode::RuntimeHalted,
                )
            })?;
            let instance_alias = lock(
                &self.registered_instances,
                HostLocation::ResolvePerformancePipelineInstance,
            )?
            .get(instance_id)
            .map(|instance| instance.instance_alias.clone())
            .ok_or_else(|| {
                RuntimeHostError::fatal(
                    HostCode::PerformancePipelineInstanceUnknown,
                    HostLocation::ObservePerformancePipelineEvent,
                    RuntimeErrorCode::RuntimeHalted,
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
            lock(
                &self.performance,
                HostLocation::ObservePerformancePipelineEvent,
            )?
            .observe_pipeline_event(observation)
        })();
        let semantic_events = match result {
            Ok(mut events) => {
                let mut recovered = lock(
                    &self.performance,
                    HostLocation::RecoverPerformancePipelineMonitor,
                )?
                .record_pipeline_success(event.timestamp_unix_ms())?;
                events.append(&mut recovered);
                events
            }
            Err(error) => {
                let tick = lock(
                    &self.performance,
                    HostLocation::DegradePerformancePipelineMonitor,
                )?
                .record_monitor_failure(
                    event.timestamp_unix_ms(),
                    error.outcome(),
                    Some(*event.event_id()),
                )?;
                tick.events
            }
        };
        self.record_performance_events(&semantic_events)
    }

    /// Workflow #385 P-m: what the shadow measurement reads from the `performance` section in
    /// effect (the defaults file's for a host started without actingd's manifest). The section
    /// passed its whole-section check when it was assembled or changed, so a refusal here is an
    /// invariant breach.
    fn performance_measurement(
        &self,
    ) -> RuntimeHostResult<actingcommand_runtime_config::performance::PerformanceMeasurement> {
        lock(
            &self.runtime_config,
            HostLocation::ReadPerformanceMeasurement,
        )?
        .performance_measurement()
        .map_err(|failure| {
            RuntimeHostError::fatal(
                failure.failure_code(),
                HostLocation::ReadPerformanceMeasurement,
                RuntimeErrorCode::RuntimeHalted,
            )
            .with_outcome_values(|outcome| failure.with_values(outcome))
        })
    }

    /// Workflow #385 P-m: the registered instances' aliases, the shadow measurement's rows.
    fn pacing_instances(&self) -> RuntimeHostResult<Vec<String>> {
        Ok(lock(
            &self.registered_instances,
            HostLocation::ReadPerformanceMeasurement,
        )?
        .values()
        .map(|instance| instance.instance_alias.clone())
        .collect())
    }

    /// Workflow #385 P-m (PERF §3.2): a frame's acquisition (`capture.completed`) or an input's
    /// backend response (`input.committed`, a tap or a gesture) of a registered instance, as a
    /// sample of the shadow measurement. An event of no registered instance gives none.
    fn record_pacing_sample_of(&self, event: &PersistedEvent) -> RuntimeHostResult<()> {
        let measured = match event.payload() {
            EventPayload::Input(InputPayload::Committed(payload)) => {
                payload.touch_response_us().map(|value| (value, true))
            }
            EventPayload::Capture(CapturePayload::Completed(payload)) => {
                payload.capture_acquire_us().map(|value| (value, false))
            }
            _ => None,
        };
        let Some((value_us, input)) = measured else {
            return Ok(());
        };
        let Some(instance_id) = event.links().instance_id() else {
            return Ok(());
        };
        let Some(instance_alias) = lock(
            &self.registered_instances,
            HostLocation::ObservePerformancePipelineEvent,
        )?
        .get(instance_id)
        .map(|instance| instance.instance_alias.clone()) else {
            return Ok(());
        };
        let source = if input {
            PacingSampleSource::TouchResponse {
                instance_alias,
                tap: event.payload().action() == EventAction::InputTap,
            }
        } else {
            PacingSampleSource::CaptureAcquire { instance_alias }
        };
        lock(
            &self.performance,
            HostLocation::ObservePerformancePipelineEvent,
        )?
        .record_pacing_sample(PacingSample {
            observed_at_unix_ms: event.timestamp_unix_ms(),
            source,
            value_us,
        });
        Ok(())
    }

    /// Workflow #385 P-m (PERF §3.5): a passed backend open of a registered instance names the
    /// backend its capture and touch baselines are kept for (a Nemu pair both).
    fn observe_pacing_backend(&self, event: &PersistedEvent) -> RuntimeHostResult<()> {
        let EventPayload::Runtime(RuntimePayload::LifecycleObserved(payload)) = event.payload()
        else {
            return Ok(());
        };
        let Some(report) = payload.backend_open() else {
            return Ok(());
        };
        let Some(selected) = report.selected.as_deref() else {
            return Ok(());
        };
        if report.status != actingcommand_contract::BackendObservationStatus::Passed {
            return Ok(());
        }
        let roles: &[PacingBackendRole] = match report.entry {
            actingcommand_contract::BackendOpenEntry::Capture => &[PacingBackendRole::Capture],
            actingcommand_contract::BackendOpenEntry::Input => &[PacingBackendRole::Touch],
            actingcommand_contract::BackendOpenEntry::NemuPair => {
                &[PacingBackendRole::Capture, PacingBackendRole::Touch]
            }
        };
        let Some(instance_id) = event.links().instance_id() else {
            return Ok(());
        };
        let Some(instance_alias) = lock(
            &self.registered_instances,
            HostLocation::ObservePerformanceBackend,
        )?
        .get(instance_id)
        .map(|instance| instance.instance_alias.clone()) else {
            return Ok(());
        };
        let mut performance = lock(&self.performance, HostLocation::ObservePerformanceBackend)?;
        for role in roles {
            performance.note_pacing_backend(&instance_alias, *role, selected);
        }
        Ok(())
    }

    /// `RuntimeOperation::PerformanceStatus` (Workflow #385 P-m, PERF §4.6; register §1.61
    /// W15): the shadow measurement's rows, read from memory. Nothing is read from or appended
    /// to the ledger.
    pub(super) fn performance_status_read(&self) -> Result<OperationSuccess, RequestFailure> {
        let settings = self.performance_measurement()?;
        let instances = self.pacing_instances()?;
        let now = self.clock.sample()?.unix_ms;
        let status = lock(&self.performance, HostLocation::ReadPerformanceStatus)?
            .pacing_status(now, &settings, &instances);
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: None,
            result: RuntimeResult::PerformanceStatus {
                status: Box::new(status),
            },
            failure_outcome: None,
        })
    }

    /// #385 P-k (PERF v1.4.2 §4.6, §5.6 item 2): the pacing factor in effect for
    /// `instance_alias`, which a contained run hands each of its steps (`step_pacing()`): the
    /// last evaluation's, 1 000 under `observe`, before the first evaluation, without counters
    /// or without a row. It is read under the performance lock alone, the last lock the status
    /// read takes; a poisoned lock is fatal.
    pub(super) fn pacing_factor_milli(&self, instance_alias: &str) -> RuntimeHostResult<u32> {
        #[cfg(test)]
        if let Some(factor_milli) = lock(
            &self.pacing_factor_pins_for_test,
            crate::codes::test_spellings::TestLocation::PinPacingFactorForTest,
        )?
        .get(instance_alias)
        .copied()
        {
            return Ok(factor_milli);
        }
        Ok(
            lock(&self.performance, HostLocation::ReadPerformanceStatus)?
                .pacing_factor_milli(instance_alias),
        )
    }

    /// Workflow #385 P-m: whether the host timer probe runs, and how: only beside an enabled
    /// monitor and with `host_probe.timer.enabled`.
    pub(super) fn timer_probe_settings(
        &self,
    ) -> RuntimeHostResult<
        Option<actingcommand_runtime_config::performance::responsiveness::HostProbeSettings>,
    > {
        if !lock(&self.performance, HostLocation::RunPerformanceProbe)?.counters_enabled() {
            return Ok(None);
        }
        let timer = self.performance_measurement()?.responsiveness.timer;
        Ok(timer.signal.enabled.then_some(timer))
    }

    /// Workflow #385 P-m (PERF §3.2, §11): a host probe that could not run; its signal stays
    /// unknown and the status read carries `perf_probe_unavailable`.
    pub(super) fn performance_probe_unavailable(
        &self,
        probe: ProbeKind,
        cause: Outcome,
    ) -> RuntimeHostResult<()> {
        let outcome = Outcome::new(PacingCode::ProbeUnavailable)
            .probe_kind(probe)
            .caused_by(cause);
        lock(&self.performance, HostLocation::RunPerformanceProbe)?
            .pacing_probe_unavailable(probe, outcome);
        Ok(())
    }
}

/// Workflow #385 P-m (PERF §3.2): the host timer probe. Every `interval_ms` it sleeps `burst`
/// times for `period_ms` and hands each oversleep, in microseconds, to the shadow measurement:
/// the time this thread waited for the processor after its timer fired. It reads no device and
/// writes nothing to the ledger. A failure ends it with `perf_probe_unavailable`; the Runtime
/// goes on.
pub(super) fn performance_probe_loop(
    shared: Arc<HostShared>,
    probe: actingcommand_runtime_config::performance::responsiveness::HostProbeSettings,
) -> RuntimeHostResult<()> {
    let period = Duration::from_millis(probe.period_ms.unwrap_or(0));
    let interval = Duration::from_millis(probe.interval_ms);
    let slice = Duration::from_millis(100);
    while !shared.fatal.is_shutdown_requested() {
        let started = std::time::Instant::now();
        let mut oversleeps = Vec::new();
        for _ in 0..probe.burst {
            let before = std::time::Instant::now();
            thread::sleep(period);
            let overslept = before.elapsed().saturating_sub(period);
            oversleeps.push(u64::try_from(overslept.as_micros()).unwrap_or(u64::MAX));
        }
        let recorded = unix_ms_now().and_then(|observed_at_unix_ms| {
            let mut performance = lock(
                &shared.performance,
                HostLocation::RecordPerformanceProbeSample,
            )?;
            for value_us in oversleeps {
                performance.record_pacing_sample(PacingSample {
                    observed_at_unix_ms,
                    source: PacingSampleSource::Probe(ProbeKind::Timer),
                    value_us,
                });
            }
            Ok(())
        });
        if let Err(error) = recorded {
            shared.performance_probe_unavailable(ProbeKind::Timer, error.outcome())?;
            return Ok(());
        }
        // The rest of the interval, in slices, so that a shutdown is seen within one.
        let mut rest = interval.saturating_sub(started.elapsed());
        while !rest.is_zero() && !shared.fatal.is_shutdown_requested() {
            let nap = rest.min(slice);
            thread::sleep(nap);
            rest = rest.saturating_sub(nap);
        }
    }
    Ok(())
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
            continue;
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
