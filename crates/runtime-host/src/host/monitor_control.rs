// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

const MONITOR_POLL_INTERVAL: Duration = Duration::from_millis(25);

const MAX_MONITOR_PROBES_PER_TICK: usize = 16;

struct MonitorRecoveryAdmission {
    reason: MonitorRecoveryCoordinationReason,
    lease_id: Option<LeaseId>,
}

impl MonitorRecoveryAdmission {
    fn admitted(&self) -> bool {
        self.reason == MonitorRecoveryCoordinationReason::SchedulerAvailable
    }
}

impl HostShared {
    pub(super) fn configure_monitor(
        &self,
        original: &RuntimeRequest,
        request: &ValidatedRuntimeRequest<'_>,
        instance_alias: &str,
        policy: RuntimeMonitorPolicy,
    ) -> Result<OperationSuccess, RequestFailure> {
        let resolved = self.resolve_instance(instance_alias)?;
        let links = self.append_client_command_intent(
            original,
            request,
            resolved.instance_id(),
            EventAction::MonitorConfigure,
            None,
        )?;
        let mut registry = lock(&self.monitor_registry, "configure_monitor_registry")?;
        let update = match registry.prepare_configure(
            instance_alias,
            policy,
            unix_ms_now().map_err(RequestFailure::from)?,
        ) {
            Ok(update) => update,
            Err(error) => {
                return Err(self.monitor_mutation_failure(
                    links,
                    EventAction::MonitorConfigure,
                    error,
                )?);
            }
        };
        self.monitor_mutation_success(
            &mut registry,
            links,
            EventAction::MonitorConfigure,
            update,
            |status| RuntimeResult::MonitorConfigured { status },
        )
    }

    pub(super) fn clear_monitor(
        &self,
        original: &RuntimeRequest,
        request: &ValidatedRuntimeRequest<'_>,
        instance_alias: &str,
    ) -> Result<OperationSuccess, RequestFailure> {
        let resolved = self.resolve_instance(instance_alias)?;
        let links = self.append_client_command_intent(
            original,
            request,
            resolved.instance_id(),
            EventAction::MonitorClear,
            None,
        )?;
        let mut registry = lock(&self.monitor_registry, "clear_monitor_registry")?;
        let update = match registry.prepare_clear(instance_alias) {
            Ok(update) => update,
            Err(error) => {
                return Err(self.monitor_mutation_failure(
                    links,
                    EventAction::MonitorClear,
                    error,
                )?);
            }
        };
        self.monitor_mutation_success(
            &mut registry,
            links,
            EventAction::MonitorClear,
            update,
            |status| RuntimeResult::MonitorCleared { status },
        )
    }

    fn monitor_mutation_success(
        &self,
        registry: &mut MonitorRegistry,
        links: EventLinksDraft,
        action: EventAction,
        update: MonitorUpdate,
        result: impl FnOnce(actingcommand_contract::RuntimeMonitorInstanceStatus) -> RuntimeResult,
    ) -> Result<OperationSuccess, RequestFailure> {
        let effect = if update.changed {
            EffectDisposition::Performed
        } else {
            EffectDisposition::NotPerformed
        };
        let event = self.append_event(
            EventSeverity::Info,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            links,
            CommandPayloadDraft::validated_runtime_state(
                action,
                effect,
                update.fact,
                AuditInput::new(),
            ),
        )?;
        registry
            .apply(&event)
            .map_err(RequestFailure::poison_without_terminal)?;
        let Some(actingcommand_contract::RuntimeStateFact::MonitorChanged { change, .. }) =
            event.payload().runtime_state()
        else {
            return Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    "monitor_committed_state_missing",
                    "commit_monitor_update",
                    RuntimeErrorCode::LedgerFailure,
                ),
            ));
        };
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: Some(terminal(&event)),
            result: result(change.status.clone()),
        })
    }

    fn monitor_mutation_failure(
        &self,
        links: EventLinksDraft,
        action: EventAction,
        error: RuntimeHostError,
    ) -> Result<RequestFailure, RequestFailure> {
        let event = self.append_event(
            EventSeverity::Error,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            links,
            CommandPayloadDraft::rejected(
                action,
                DiagnosticCode::RuntimeDiagnostic,
                EffectDisposition::Indeterminate,
                AuditInput::new(),
            ),
        )?;
        Ok(RequestFailure::poison(error, Some(terminal(&event))))
    }

    fn run_monitor_probe(&self, probe: &DueMonitorProbe) -> RuntimeHostResult<()> {
        let started_at_unix_ms = unix_ms_now()?;
        let instance = self.monitor_instance(&probe.instance_alias)?;
        let instance_guard = self
            .instance_guard(instance.instance_id())
            .map_err(|failure| *failure.error)?;
        let admission = lock(&instance_guard, "lock_instance_admission")?;
        if instance.provenance() != ExecutionBackendProvenance::PhysicalDevice {
            return Err(RuntimeHostError::fatal(
                "fixture_monitor_scope_forbidden",
                "run_monitor_probe",
                RuntimeErrorCode::RuntimeFatal,
            ));
        }
        let issued = self
            .events
            .issuer()
            .issue_monitor_probe(instance.instance_id())
            .map_err(|_| {
                RuntimeHostError::fatal(
                    "monitor_probe_id_issue_failed",
                    "run_monitor_probe",
                    RuntimeErrorCode::RuntimeFatal,
                )
            })?;
        let links = issued.event_links();
        self.append_event_raw(
            EventSeverity::Info,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            links.clone(),
            MonitorPayloadDraft::requested(AuditInput::new()),
        )?;
        self.append_event_raw(
            EventSeverity::Info,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            links.clone(),
            MonitorPayloadDraft::started(AuditInput::new()),
        )?;
        self.append_event_raw(
            EventSeverity::Info,
            EventSource::Device,
            OriginModule::Capture,
            EventActor::Runtime,
            links.clone(),
            CapturePayloadDraft::requested(EventAction::CaptureObserve, AuditInput::new()),
        )?;
        self.append_event_raw(
            EventSeverity::Info,
            EventSource::Runtime,
            OriginModule::Recognition,
            EventActor::Runtime,
            links.clone(),
            RecognitionPayloadDraft::requested(EventAction::RecognitionObserve, AuditInput::new()),
        )?;

        let registration = self.mark_resources_in_use()?;
        let frame = match self
            .execution
            .capture_retained_with_registration_guard(&probe.instance_alias, registration)
        {
            Ok(frame) => frame,
            Err(error) => {
                let error =
                    self.finish_capture_failure_while_guarded(error, links.clone(), &admission)?;
                let error = RuntimeHostError::execution("run_monitor_capture", &error);
                if self.retain_unconfirmed_resources(&error, links.clone())? {
                    return Err(error);
                }
                return self.finish_monitor_failure(probe, &links, started_at_unix_ms, error, true);
            }
        };
        let artifact_png = match frame.png_for_artifact() {
            Ok(png) => png,
            Err(_) => {
                let error = RuntimeHostError::request(
                    "capture_frame_invalid",
                    "run_monitor_capture",
                    RuntimeErrorCode::CaptureFailed,
                );
                return self.finish_monitor_failure(probe, &links, started_at_unix_ms, error, true);
            }
        };
        let write_context =
            ArtifactWriteContext::new(issued.artifact_links(), links.clone(), unix_ms_now()?);
        let mut sink = RuntimeArtifactEventSink {
            ledger: &self.ledger,
            events: &self.events,
        };
        self.artifacts
            .put(
                ArtifactWriteRequest::new(
                    ArtifactKind::CaptureFrame,
                    &artifact_png,
                    write_context,
                    ArtifactIssuePolicy::new(
                        ArtifactProducer::CaptureStore,
                        RetentionClass::Adaptive,
                        ArtifactRedactionState::NotRequired,
                    ),
                ),
                &mut sink,
            )
            .map_err(RuntimeHostError::artifact)?;
        self.append_event_raw(
            EventSeverity::Info,
            EventSource::Device,
            OriginModule::Capture,
            EventActor::Runtime,
            links.clone(),
            CapturePayloadDraft::completed(
                EventAction::CaptureObserve,
                EffectDisposition::Performed,
                frame.width,
                frame.height,
                AuditInput::new(),
            ),
        )?;

        let observation = match self.execution.observe_monitor(
            &probe.instance_alias,
            probe.policy.expected_page(),
            &frame,
        ) {
            Ok(observation) => observation,
            Err(error) => {
                let error = RuntimeHostError::execution("classify_monitor_observation", &error);
                return self.finish_monitor_failure(
                    probe,
                    &links,
                    started_at_unix_ms,
                    error,
                    false,
                );
            }
        };
        let decision =
            decide_monitor(probe.policy.decision_policy(), &observation).map_err(|_| {
                RuntimeHostError::fatal(
                    "monitor_decision_invalid",
                    "run_monitor_probe",
                    RuntimeErrorCode::RuntimeFatal,
                )
            })?;
        self.append_event_raw(
            EventSeverity::Info,
            EventSource::Runtime,
            OriginModule::Recognition,
            EventActor::Runtime,
            links.clone(),
            RecognitionPayloadDraft::completed(
                EventAction::RecognitionObserve,
                EffectDisposition::Performed,
                frame.width,
                frame.height,
                RecognitionVerdict::FrameDecoded,
                AuditInput::new(),
            ),
        )?;
        let mut registry = lock(&self.monitor_registry, "complete_monitor_probe")?;
        let update = registry.prepare_completion(
            probe,
            started_at_unix_ms,
            unix_ms_now()?,
            decision.clone(),
        )?;
        let completed = self.append_event_raw(
            EventSeverity::Info,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            links,
            MonitorPayloadDraft::completed(
                EffectDisposition::Performed,
                observation,
                decision.clone(),
                AuditInput::new(),
            )
            .with_runtime_state(update.fact)
            .map_err(|_| {
                RuntimeHostError::fatal(
                    "monitor_state_invalid",
                    "complete_monitor_probe",
                    RuntimeErrorCode::RuntimeFatal,
                )
            })?,
        )?;
        registry.apply(&completed)?;
        drop(registry);
        if update.changed {
            self.record_monitor_recovery_coordination(&instance, &issued, &decision)?;
        }
        Ok(())
    }

    fn record_monitor_recovery_coordination(
        &self,
        instance: &RegisteredInstance,
        issued: &IssuedMonitorProbe,
        decision: &actingcommand_contract::MonitorDecision,
    ) -> RuntimeHostResult<()> {
        let Some(recovery) = decision.recovery() else {
            return Ok(());
        };
        let admission = self.monitor_recovery_admission(instance.instance_id())?;
        let links = admission.lease_id.map_or_else(
            || issued.event_links(),
            |lease_id| issued.event_links_with_lease(lease_id),
        );
        let (severity, payload) = if admission.admitted() {
            (
                EventSeverity::Info,
                MonitorPayloadDraft::recovery_admitted(recovery, AuditInput::new()),
            )
        } else {
            (
                EventSeverity::Warning,
                MonitorPayloadDraft::recovery_deferred(
                    recovery,
                    admission.reason,
                    AuditInput::new(),
                ),
            )
        };
        self.append_event_raw(
            severity,
            EventSource::Scheduler,
            OriginModule::Scheduler,
            EventActor::Scheduler,
            links,
            payload,
        )?;
        Ok(())
    }

    fn monitor_recovery_admission(
        &self,
        instance_id: InstanceId,
    ) -> RuntimeHostResult<MonitorRecoveryAdmission> {
        let now = self.monotonic_ms()?;
        let scheduler = lock(&self.scheduler, "coordinate_monitor_recovery")?;
        if let Some(active) = scheduler.active_lease(instance_id) {
            let token = active.token();
            if token.owner_epoch() != self.owner_epoch || token.instance_id() != instance_id {
                return Err(RuntimeHostError::fatal(
                    "monitor_recovery_fencing_state_invalid",
                    "coordinate_monitor_recovery",
                    RuntimeErrorCode::RuntimeFatal,
                ));
            }
            let reason = if token.expires_at_monotonic_ms() <= now {
                MonitorRecoveryCoordinationReason::LeaseExpired
            } else if active.destructive_step_active() {
                MonitorRecoveryCoordinationReason::DestructiveStepActive
            } else if active.preempt_requested() {
                MonitorRecoveryCoordinationReason::PreemptionPending
            } else {
                MonitorRecoveryCoordinationReason::ActiveLease
            };
            return Ok(MonitorRecoveryAdmission {
                reason,
                lease_id: Some(token.lease_id()),
            });
        }
        let reason = if scheduler.cooldown_active(instance_id, now) {
            MonitorRecoveryCoordinationReason::TakeoverCooldown
        } else if scheduler.queued_count(instance_id) > 0 {
            MonitorRecoveryCoordinationReason::QueuedLeaseRequests
        } else {
            MonitorRecoveryCoordinationReason::SchedulerAvailable
        };
        Ok(MonitorRecoveryAdmission {
            reason,
            lease_id: None,
        })
    }

    fn finish_monitor_failure(
        &self,
        probe: &DueMonitorProbe,
        links: &EventLinksDraft,
        started_at_unix_ms: u64,
        error: RuntimeHostError,
        capture_failed: bool,
    ) -> RuntimeHostResult<()> {
        let runtime_code = error.projection().code;
        let diagnostic = if capture_failed {
            DiagnosticCode::CaptureFailed
        } else {
            DiagnosticCode::RecognitionFailed
        };
        if capture_failed {
            let payload = CapturePayloadDraft::failed_with_causes(
                EventAction::CaptureObserve,
                diagnostic,
                EffectDisposition::NotPerformed,
                error.diagnostic_detail().cloned(),
                error.cleanup_cause().cloned(),
                AuditInput::new(),
            );
            let failed = self.append_event_raw(
                EventSeverity::Error,
                EventSource::Device,
                OriginModule::Capture,
                EventActor::Runtime,
                links.clone(),
                payload,
            )?;
            self.record_required_failure(&error, &failed, links.clone())?;
        }
        self.append_event_raw(
            EventSeverity::Error,
            EventSource::Runtime,
            OriginModule::Recognition,
            EventActor::Runtime,
            links.clone(),
            RecognitionPayloadDraft::failed(
                EventAction::RecognitionObserve,
                diagnostic,
                EffectDisposition::NotPerformed,
                AuditInput::new(),
            ),
        )?;
        let mut registry = lock(&self.monitor_registry, "fail_monitor_probe")?;
        let update =
            registry.prepare_failure(probe, started_at_unix_ms, unix_ms_now()?, runtime_code)?;
        let failed = self.append_event_raw(
            EventSeverity::Error,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            links.clone(),
            MonitorPayloadDraft::failed(
                diagnostic,
                EffectDisposition::NotPerformed,
                AuditInput::new(),
            )
            .with_runtime_state(update.fact)
            .map_err(|_| {
                RuntimeHostError::fatal(
                    "monitor_state_invalid",
                    "fail_monitor_probe",
                    RuntimeErrorCode::RuntimeFatal,
                )
            })?,
        )?;
        registry.apply(&failed)?;
        if error.code() == "monitor_observation_invalid" {
            return Err(error);
        }
        Ok(())
    }

    fn monitor_instance(&self, instance_alias: &str) -> RuntimeHostResult<RegisteredInstance> {
        let registered = lock(&self.registered_instances, "resolve_monitor_instance")?
            .values()
            .find(|instance| instance.instance_alias == instance_alias)
            .cloned()
            .ok_or_else(|| {
                RuntimeHostError::fatal(
                    "monitor_instance_unknown",
                    "resolve_monitor_instance",
                    RuntimeErrorCode::RuntimeFatal,
                )
            })?;
        let resolved = self
            .execution
            .resolve(instance_alias)
            .map_err(|error| RuntimeHostError::execution("resolve_monitor_instance", &error))?;
        if resolved.instance_id() != registered.instance_id
            || resolved.audit_endpoint() != registered.audit_endpoint
            || resolved.provenance() != registered.provenance
        {
            return Err(RuntimeHostError::fatal(
                "runtime_instance_identity_mismatch",
                "resolve_monitor_instance",
                RuntimeErrorCode::RuntimeFatal,
            ));
        }
        Ok(registered)
    }
}

pub(super) fn monitor_probe_loop(shared: Arc<HostShared>) -> RuntimeHostResult<()> {
    while !shared.fatal.is_shutdown_requested() {
        let now_unix_ms = unix_ms_now()?;
        let due = lock(&shared.monitor_registry, "read_due_monitors")?
            .due(now_unix_ms, MAX_MONITOR_PROBES_PER_TICK)?;
        for probe in due {
            if shared.fatal.is_shutdown_requested() {
                return Ok(());
            }
            let Some(_work) = shared.begin_work()? else {
                return Ok(());
            };
            if let Err(error) = shared.run_monitor_probe(&probe) {
                shared.fatal.mark(error.clone())?;
                return Err(error);
            }
        }
        thread::sleep(MONITOR_POLL_INTERVAL);
    }
    Ok(())
}
