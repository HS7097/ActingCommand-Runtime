// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

const MONITOR_POLL_INTERVAL: Duration = Duration::from_millis(25);

const MAX_MONITOR_PROBES_PER_TICK: usize = 16;

enum MonitorFailureStage {
    Capture,
    Artifact,
    Recognition,
}

/// Per-instance fence classification shared with emulator instance control.
pub(super) struct MonitorRecoveryAdmission {
    pub(super) reason: MonitorRecoveryCoordinationReason,
    lease_id: Option<LeaseId>,
}

impl MonitorRecoveryAdmission {
    pub(super) fn admitted(&self) -> bool {
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
        if let Err(error) = self.admit_capacity() {
            if error.is_fatal() {
                return Err(error);
            }
            return self.refuse_monitor_probe(probe, links, started_at_unix_ms, error);
        }
        // The probe would open the device session: a pending discovery binding refuses it
        // typed (`instance_not_running`) before any capture is requested.
        if let Err(error) = instance_not_running(&instance) {
            return self.refuse_monitor_probe(probe, links, started_at_unix_ms, error);
        }
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

        let frame_id = links.frame_id().ok_or_else(|| {
            RuntimeHostError::fatal(
                "monitor_frame_identity_missing",
                "run_monitor_capture",
                RuntimeErrorCode::RuntimeFatal,
            )
        })?;
        let frame_store = actingcommand_artifact_store::FrameStore::new(
            frame_retention::spill_root(self.artifacts.root(), frame_id)
                .map_err(RuntimeHostError::artifact)?,
            frame_retention::capture_frame_store_config(),
        )
        .map_err(RuntimeHostError::artifact)?;
        let memory = frame_store.memory_budget();
        let registration = self.mark_resources_in_use()?;
        let captured = self.execution.capture_retained_with_registration_guard(
            &probe.instance_alias,
            registration,
            memory.clone(),
        );
        let mut frame = match captured {
            Ok(mut frame) => {
                self.append_backend_open_observations(
                    &std::mem::take(&mut frame.backend_open_observations),
                    links.clone(),
                    EventSource::Device,
                    OriginModule::Capture,
                )?;
                frame
            }
            Err(error) => {
                self.append_backend_open_failure_observations(
                    &error,
                    links.clone(),
                    EventSource::Device,
                    OriginModule::Capture,
                )?;
                let error =
                    self.finish_capture_failure_while_guarded(error, links.clone(), &admission)?;
                let error = RuntimeHostError::execution("run_monitor_capture", &error);
                if self.retain_unconfirmed_resources(&error, links.clone())? {
                    return Err(error);
                }
                return self.finish_monitor_failure(
                    probe,
                    &links,
                    started_at_unix_ms,
                    error,
                    MonitorFailureStage::Capture,
                );
            }
        };
        let prepared = (|| -> actingcommand_device::DeviceResult<()> {
            frame.validate_layout()?;
            let required = frame.artifact_png_workspace_bytes()?;
            let mut workspace = memory.reserve(required)?;
            if let std::borrow::Cow::Owned(png) = frame.png_for_artifact_with_budget(required)? {
                frame.retain_admitted_png(png, &mut workspace)?;
            }
            Ok(())
        })();
        if let Err(error) = prepared {
            let error = if error.frame_memory_failure().is_some() {
                RuntimeHostError::artifact(ArtifactStoreError::incoming_frame(error))
            } else {
                RuntimeHostError::request(
                    "capture_frame_invalid",
                    "run_monitor_capture",
                    RuntimeErrorCode::CaptureFailed,
                )
            };
            if error.is_fatal() {
                return Err(error);
            }
            return self.finish_monitor_failure(
                probe,
                &links,
                started_at_unix_ms,
                error,
                MonitorFailureStage::Capture,
            );
        }
        self.append_event_raw(
            EventSeverity::Info,
            EventSource::Device,
            OriginModule::Capture,
            EventActor::Runtime,
            links.clone(),
            CapturePayloadDraft::completed_with_capture_acquire(
                EventAction::CaptureObserve,
                EffectDisposition::Performed,
                frame.width,
                frame.height,
                frame.capture_acquire_us(),
                AuditInput::new(),
            ),
        )?;
        let write_context =
            ArtifactWriteContext::new(issued.artifact_links(), links.clone(), unix_ms_now()?);
        let mut sink = RuntimeArtifactEventSink {
            ledger: &self.ledger,
            events: &self.events,
        };
        let persisted = (|| -> ArtifactStoreResult<()> {
            let mut pipeline = CapturePipeline::open_with_frame_store(
                Arc::clone(&self.artifacts),
                frame_store,
                CapturePipelineConfig {
                    frame_store: frame_retention::capture_frame_store_config(),
                    retention_class: RetentionClass::Adaptive,
                    redaction_state: ArtifactRedactionState::NotRequired,
                    ..CapturePipelineConfig::default()
                },
                write_context.clone(),
                &mut sink,
            )?;
            pipeline.with_frame_copy(&frame, |pipeline, frame| -> ArtifactStoreResult<()> {
                pipeline.record_frame(
                    FrameStoreFrameInput {
                        frame_index: 0,
                        file_name: "frame-0.png".to_owned(),
                        label: "initial".to_owned(),
                        recognition_state: RecognitionState::Pending,
                        pinned_reason: None,
                        frame,
                    },
                    write_context,
                    &mut sink,
                )?;
                pipeline.persist_frame(0, &mut sink)?;
                pipeline.cleanup_spills()?;
                Ok(())
            })?
        })();
        if let Err(error) = persisted {
            let error = RuntimeHostError::artifact(error);
            if error.is_fatal() {
                return Err(error);
            }
            return self.finish_monitor_failure(
                probe,
                &links,
                started_at_unix_ms,
                error,
                MonitorFailureStage::Artifact,
            );
        }

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
                    MonitorFailureStage::Recognition,
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

    /// A probe refused before it started (no capacity, or the instance is not running):
    /// `monitor.failed` with the refusal's runtime code plus the `runtime.failed` record.
    fn refuse_monitor_probe(
        &self,
        probe: &DueMonitorProbe,
        links: EventLinksDraft,
        started_at_unix_ms: u64,
        error: RuntimeHostError,
    ) -> RuntimeHostResult<()> {
        let mut registry = lock(&self.monitor_registry, "refuse_monitor_probe")?;
        let update = registry.prepare_failure(
            probe,
            started_at_unix_ms,
            unix_ms_now()?,
            error.projection().code,
        )?;
        let failed = self.append_event_raw(
            EventSeverity::Warning,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            links.clone(),
            MonitorPayloadDraft::failed(
                DiagnosticCode::RuntimeDiagnostic,
                EffectDisposition::NotPerformed,
                AuditInput::new(),
            )
            .with_runtime_state(update.fact)
            .map_err(|_| {
                RuntimeHostError::fatal(
                    "monitor_state_invalid",
                    "refuse_monitor_probe",
                    RuntimeErrorCode::RuntimeFatal,
                )
            })?,
        )?;
        registry.apply(&failed)?;
        drop(registry);
        self.record_required_failure(&error, &failed, links)
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

    pub(super) fn monitor_recovery_admission(
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
        stage: MonitorFailureStage,
    ) -> RuntimeHostResult<()> {
        let runtime_code = error.projection().code;
        let diagnostic = match stage {
            MonitorFailureStage::Capture => DiagnosticCode::CaptureFailed,
            MonitorFailureStage::Artifact => DiagnosticCode::RuntimeDiagnostic,
            MonitorFailureStage::Recognition => DiagnosticCode::RecognitionFailed,
        };
        if matches!(stage, MonitorFailureStage::Capture) {
            let payload = CapturePayloadDraft::failed_with_causes(
                EventAction::CaptureObserve,
                diagnostic,
                if matches!(
                    error.code(),
                    "capture_frame_invalid" | "frame_workspace_unavailable"
                ) {
                    EffectDisposition::Indeterminate
                } else {
                    EffectDisposition::NotPerformed
                },
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
                if matches!(stage, MonitorFailureStage::Artifact) {
                    EffectDisposition::Performed
                } else {
                    EffectDisposition::NotPerformed
                },
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
        drop(registry);
        if matches!(stage, MonitorFailureStage::Artifact) {
            self.record_required_failure(&error, &failed, links.clone())?;
        }
        if error.code() == "monitor_observation_invalid"
            || (error.is_fatal()
                && matches!(
                    error.code(),
                    "frame_workspace_unavailable"
                        | "frame_memory_owner_missing_or_mismatched"
                        | "frame_memory_accounting_invalid"
                        | "frame_memory_budget_source_failed"
                ))
        {
            return Err(error);
        }
        Ok(())
    }

    fn monitor_instance(&self, instance_alias: &str) -> RuntimeHostResult<RegisteredInstance> {
        // The identity check runs under the registry lock so an endpoint rebinding is never
        // observed half-applied.
        let registry = lock(&self.registered_instances, "resolve_monitor_instance")?;
        let registered = registry
            .values()
            .find(|instance| instance.instance_alias == instance_alias)
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
        Ok(registered.clone())
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
