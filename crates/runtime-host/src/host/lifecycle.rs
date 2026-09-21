// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

impl RuntimeLifecycleFailureStage {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::PolicyInitialization => "runtime.lifecycle.policy_initialization",
            Self::PolicyMonitor => "runtime.lifecycle.policy_monitor",
            Self::PolicyForward => "runtime.lifecycle.policy_forward",
            Self::StrategicReport => "runtime.lifecycle.strategic_report",
            Self::SessionClose => "runtime.lifecycle.session_close",
            Self::OperationCleanup => "runtime.lifecycle.operation_cleanup",
            Self::ConnectionCleanup => "runtime.lifecycle.connection_cleanup",
            Self::ShutdownJoin => "runtime.lifecycle.shutdown_join",
            Self::InfoFileRemoval => "runtime.lifecycle.info_file_removal",
            Self::RetainedReference => "runtime.lifecycle.retained_reference",
            Self::HostClose => "runtime.lifecycle.host_close",
            Self::PolicyDriver => "runtime.lifecycle.policy_driver",
            Self::PolicyControl => "runtime.lifecycle.policy_control",
            Self::PolicyBootstrap => "runtime.lifecycle.policy_bootstrap",
        }
    }
}

impl HostShared {
    pub(super) fn append_lifecycle_observed(
        &self,
        phase: RuntimeLifecyclePhase,
        links: EventLinksDraft,
    ) -> RuntimeHostResult<EventId> {
        self.append_event_raw(
            EventSeverity::Info,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            links,
            RuntimePayloadDraft::lifecycle_observed(self.owner_epoch, phase, AuditInput::new()),
        )
        .map(|event| *event.event_id())
        .map_err(|_| ledger_error("append_runtime_lifecycle_observed"))
    }

    pub(super) fn append_stdio_close_observations(
        &self,
        observations: &[actingcommand_execution_kernel::ExecutionStdioObservation],
        instance_id: Option<InstanceId>,
        links: EventLinksDraft,
    ) -> RuntimeHostResult<()> {
        if observations.is_empty() {
            return Ok(());
        }
        if self.lifecycle_append_failed.load(Ordering::Acquire) {
            return Err(ledger_error("append_vendor_stdio_close"));
        }
        let gate = lock(&self.fact_write_gate, "append_vendor_stdio_close")?;
        if self.lifecycle_append_failed.load(Ordering::Acquire) {
            return Err(ledger_error("append_vendor_stdio_close"));
        }
        let mut persisted = Vec::new();
        for observation in observations {
            if observation.recorded_event.get().is_some() {
                continue;
            }
            let residual = observation.facts.paths.iter().any(|path| {
                matches!(
                    path.removal,
                    actingcommand_contract::StdioPathRemoval::Residual(_)
                )
            });
            let event = self
                .append_event_under_fact_gate(
                    if residual {
                        EventSeverity::Warning
                    } else {
                        EventSeverity::Info
                    },
                    EventSource::Runtime,
                    OriginModule::Runtime,
                    EventActor::Runtime,
                    links.clone(),
                    RuntimePayloadDraft::vendor_stdio_close(
                        self.owner_epoch,
                        instance_id,
                        (*observation.facts).clone(),
                    ),
                )
                .map_err(|_| {
                    self.lifecycle_append_failed.store(true, Ordering::Release);
                    ledger_error("append_vendor_stdio_close")
                })?;
            persisted.push((observation, event));
        }
        if !persisted.is_empty() {
            self.synchronize_fact_store_under_gate().inspect_err(|_| {
                self.lifecycle_append_failed.store(true, Ordering::Release);
            })?;
            for (observation, event) in &persisted {
                let _ = observation.recorded_event.set(*event.event_id());
            }
        }
        drop(gate);
        for (_, event) in persisted {
            self.observe_pipeline_event(&event)?;
        }
        Ok(())
    }

    pub(super) fn append_lifecycle_failure(
        &self,
        stage: RuntimeLifecycleFailureStage,
        failure: RuntimeLifecycleFailure<'_>,
        links: EventLinksDraft,
        entered_event_id: Option<EventId>,
    ) -> RuntimeHostResult<()> {
        let decision_id = match &failure {
            RuntimeLifecycleFailure::PolicyAdmission { decision_id, .. } => Some(*decision_id),
            _ => None,
        };
        let client_message = match &failure {
            RuntimeLifecycleFailure::Client { message, .. } => Some(*message),
            _ => None,
        };
        let host_error = match &failure {
            RuntimeLifecycleFailure::Host(error)
            | RuntimeLifecycleFailure::PolicyAdmission { error, .. } => Some(*error),
            _ => None,
        };
        if let Some(error) = host_error
            && error.projection().code != RuntimeErrorCode::LedgerFailure
        {
            self.append_backend_open_observations(
                error.diagnostics().backend_open_observations(),
                links.clone(),
            )?;
        }
        let failure_stage = host_error
            .and_then(|error| error.lifecycle.failure_stage)
            .unwrap_or_else(|| stage.as_str());
        if let Some(error) = host_error
            && let Some(complete) = &error.lifecycle.complete_failure
        {
            if error.projection().code == RuntimeErrorCode::LedgerFailure {
                return Err(error.clone());
            }
            self.append_lifecycle_failure(
                stage,
                match decision_id {
                    Some(decision_id) => RuntimeLifecycleFailure::PolicyAdmission {
                        error: &complete.primary,
                        decision_id,
                    },
                    None => RuntimeLifecycleFailure::Host(&complete.primary),
                },
                links.clone(),
                entered_event_id,
            )?;
            let reference = complete
                .primary
                .diagnostics()
                .recorded_event()
                .get()
                .copied()
                .or(entered_event_id);
            for (_, secondary) in &complete.secondary {
                self.append_lifecycle_failure(
                    stage,
                    match decision_id {
                        Some(decision_id) => RuntimeLifecycleFailure::PolicyAdmission {
                            error: secondary,
                            decision_id,
                        },
                        None => RuntimeLifecycleFailure::Host(secondary),
                    },
                    links.clone(),
                    reference,
                )?;
            }
            return Ok(());
        }
        if let Some(error) = host_error
            && error.diagnostics().recorded_event().get().is_some()
            && error
                .diagnostics()
                .lifecycle_causes()
                .iter()
                .all(|cause| cause.recorded_event.get().is_some())
            && error
                .diagnostics()
                .vendor_stdio()
                .iter()
                .all(|observation| observation.recorded_event.get().is_some())
        {
            return Ok(());
        }
        if self.lifecycle_append_failed.load(Ordering::Acquire) {
            return Err(ledger_error("append_runtime_lifecycle_failure"));
        }
        if let Some(error) = host_error
            && error.projection().code == RuntimeErrorCode::LedgerFailure
        {
            return Err(error.clone());
        }
        if let Some(error) = host_error {
            self.append_stdio_close_observations(
                error.diagnostics().vendor_stdio(),
                error.lifecycle.instance_id,
                links.clone(),
            )?;
        }
        let (origin, code, operation, fatal, runtime_code) = match failure {
            RuntimeLifecycleFailure::Host(error)
            | RuntimeLifecycleFailure::PolicyAdmission { error, .. } => (
                "runtime_host",
                error.code(),
                Some(error.operation()),
                Some(error.is_fatal()),
                Some(error.projection().code),
            ),
            RuntimeLifecycleFailure::Client {
                code,
                operation,
                fatal,
                runtime_code,
                message: _,
            } => (
                "runtime_client",
                code,
                Some(operation),
                Some(fatal),
                runtime_code,
            ),
            RuntimeLifecycleFailure::Process { code } => ("actingd", code, None, None, None),
        };
        let message = serde_json::to_string(&serde_json::json!({
            "origin": origin,
            "code": code,
            "operation": operation,
            "fatal": fatal,
            "runtime_code": runtime_code,
            "owner_epoch": self.owner_epoch,
            "entered_event_id": entered_event_id,
            "decision_id": decision_id,
        }))
        .map_err(|_| ledger_error("encode_runtime_lifecycle_failure"))?;
        let gate = lock(&self.fact_write_gate, "append_runtime_lifecycle_failure")?;
        let mut persisted = Vec::new();
        let mut emit = |cause: Option<&actingcommand_contract::LifecycleCauseDraft>,
                        reference,
                        client_part: Option<(&str, usize, usize)>| {
            let lifecycle = actingcommand_contract::RuntimeLifecycleFailureDraft::new(
                self.owner_epoch,
                failure_stage,
                origin,
                code,
            )
            .with_operation(operation)
            .with_projection(fatal, runtime_code)
            .with_entered_event_id(reference)
            .with_instance_id(host_error.and_then(|error| error.lifecycle.instance_id))
            .with_primary_detail(host_error.and_then(|error| error.diagnostic_detail().cloned()))
            .with_adb_recovery(
                host_error.and_then(|error| error.diagnostics().adb_recovery().cloned()),
            )
            .with_native_detail(
                client_part
                    .map(|(text, _, _)| {
                        actingcommand_contract::LifecycleNativeDetail::new(text, false)
                    })
                    .or_else(|| {
                        host_error.and_then(|error| error.diagnostics().native_detail().cloned())
                    }),
            )
            .with_capacity(host_error.and_then(|error| error.lifecycle.capacity.clone()))
            .with_task_timing(host_error.and_then(|error| error.lifecycle.task_timing.clone()))
            .with_raw_os_error(host_error.and_then(|error| error.lifecycle.raw_os_error))
            .with_cleanup_cause(host_error.and_then(|error| error.cleanup_cause().cloned()))
            .with_cause(cause.cloned());
            let cause_fatal = cause.map_or(fatal == Some(true), |cause| {
                cause.severity() == actingcommand_contract::CleanupCauseSeverity::Fatal
            });
            let event = self
                .append_event_under_fact_gate(
                    if cause_fatal {
                        EventSeverity::Fatal
                    } else {
                        EventSeverity::Error
                    },
                    EventSource::Runtime,
                    OriginModule::Runtime,
                    EventActor::Runtime,
                    links.clone(),
                    RuntimePayloadDraft::failed_with_lifecycle(
                        if decision_id.is_some() {
                            DiagnosticCode::PolicyRejected
                        } else if runtime_code == Some(RuntimeErrorCode::ProtocolInvalid) {
                            DiagnosticCode::RuntimeProtocolInvalid
                        } else {
                            DiagnosticCode::RuntimeDiagnostic
                        },
                        if decision_id.is_some() {
                            EffectDisposition::NotPerformed
                        } else {
                            EffectDisposition::Indeterminate
                        },
                        DiagnosticDetailDraft::new(
                            "runtime_lifecycle",
                            failure_stage,
                            origin,
                            operation.unwrap_or("actingd_process"),
                            match client_part {
                                Some((_, index, count)) => {
                                    let kind = if host_error.is_some() {
                                        "ppocr_message_part"
                                    } else {
                                        "client_message_part"
                                    };
                                    format!("{message}; {kind}={index}/{count}")
                                }
                                None => message.clone(),
                            },
                            Sensitivity::Internal,
                        ),
                        lifecycle,
                        AuditInput::new(),
                    ),
                )
                .map_err(|_| {
                    self.lifecycle_append_failed.store(true, Ordering::Release);
                    ledger_error("append_runtime_lifecycle_failure")
                })?;
            let id = *event.event_id();
            persisted.push(event);
            Ok::<_, RuntimeHostError>(id)
        };
        if let Some(error) = host_error {
            let phase_close = matches!(
                error.code(),
                "input_backend_close_failed" | "capture_backend_close_failed"
            ) && error.diagnostics().lifecycle_causes().iter().any(|cause| {
                cause.cause.phase() != actingcommand_contract::LifecycleFailurePhase::Retirement
            });
            if error.diagnostics().recorded_event().get().is_none() && !phase_close {
                let mut parts = Vec::new();
                let mut remaining = error.lifecycle.ppocr_message.as_deref().unwrap_or("");
                while !remaining.is_empty() {
                    let mut end = remaining.len().min(1024);
                    while !remaining.is_char_boundary(end) {
                        end -= 1;
                    }
                    parts.push(&remaining[..end]);
                    remaining = &remaining[end..];
                }
                let mut first = None;
                for (index, part) in parts.iter().enumerate() {
                    let id = emit(
                        None,
                        first.or(entered_event_id),
                        Some((part, index + 1, parts.len())),
                    )?;
                    first.get_or_insert(id);
                }
                let id = match first {
                    Some(id) => id,
                    None => emit(None, entered_event_id, None)?,
                };
                // A partial message is not a fully recorded error.
                let _ = error.diagnostics().recorded_event().set(id);
            }
            let reference =
                entered_event_id.or_else(|| error.diagnostics().recorded_event().get().copied());
            for cause in error.diagnostics().lifecycle_causes() {
                if cause.recorded_event.get().is_none() {
                    let id = emit(Some(&cause.cause), reference, None)?;
                    let _ = cause.recorded_event.set(id);
                }
            }
            if phase_close
                && let Some(id) = error
                    .diagnostics()
                    .lifecycle_causes()
                    .first()
                    .and_then(|cause| cause.recorded_event.get())
            {
                let _ = error.diagnostics().recorded_event().set(*id);
            }
        } else if let Some(text) = client_message {
            // Each original detail retains its 1024-byte schema limit. Ordered,
            // linked parts preserve the complete client display without truncation.
            let mut parts = Vec::new();
            let mut remaining = text;
            while !remaining.is_empty() {
                let mut end = remaining.len().min(1024);
                while !remaining.is_char_boundary(end) {
                    end -= 1;
                }
                parts.push(&remaining[..end]);
                remaining = &remaining[end..];
            }
            let mut reference = entered_event_id;
            for (index, part) in parts.iter().enumerate() {
                let id = emit(None, reference, Some((part, index + 1, parts.len())))?;
                reference.get_or_insert(id);
            }
            if parts.is_empty() {
                emit(None, entered_event_id, None)?;
            }
        } else {
            emit(None, entered_event_id, None)?;
        }
        if !persisted.is_empty() {
            self.synchronize_fact_store_under_gate()?;
        }
        drop(gate);
        for event in persisted {
            self.observe_pipeline_event(&event)?;
        }
        Ok(())
    }

    pub(super) fn record_lifecycle_result(
        &self,
        stage: RuntimeLifecycleFailureStage,
        slot: &mut Option<RuntimeHostError>,
        result: RuntimeHostResult<()>,
    ) {
        if let Err(error) = result {
            let error = error.with_failure_stage(stage.as_str());
            let writer_failed = slot.as_ref().is_some_and(|failure| {
                failure.projection().code == RuntimeErrorCode::LedgerFailure
            }) || error.projection().code == RuntimeErrorCode::LedgerFailure
                || self.lifecycle_append_failed.load(Ordering::Acquire);
            if writer_failed {
                self.lifecycle_append_failed.store(true, Ordering::Release);
            }
            if !writer_failed
                && let Err(append_error) = self.append_lifecycle_failure(
                    stage,
                    RuntimeLifecycleFailure::Host(&error),
                    EventLinksDraft::default(),
                    None,
                )
            {
                if append_error.projection().code == RuntimeErrorCode::LedgerFailure {
                    self.lifecycle_append_failed.store(true, Ordering::Release);
                }
                let complete = error.with_complete_failure(
                    crate::error::RuntimeFailureRelation::LifecycleRecord,
                    append_error.with_failure_stage(stage.as_str()),
                );
                record_failure(slot, Err(complete));
                return;
            }
            record_failure(slot, Err(error));
        }
    }

    pub(super) fn record_required_failure(
        &self,
        error: &RuntimeHostError,
        outcome: &PersistedEvent,
        links: EventLinksDraft,
    ) -> RuntimeHostResult<()> {
        // These original failure builders copy this error's primary and cleanup
        // details into their outcome. Extra native details still need lifecycle facts.
        let (primary_detail_recorded, cleanup_cause_recorded) = match outcome.payload() {
            EventPayload::Capture(CapturePayload::Failed(payload))
            | EventPayload::Input(InputPayload::Failed(payload)) => (
                payload.detail().is_some(),
                payload.cleanup_cause().is_some(),
            ),
            _ => (false, false),
        };
        if error.diagnostics().native_detail().is_none()
            && error.lifecycle.capacity.is_none()
            && (error.diagnostic_detail().is_none() || primary_detail_recorded)
            && (error.cleanup_cause().is_none() || cleanup_cause_recorded)
            && error.lifecycle.raw_os_error.is_none()
            && error.diagnostics().adb_recovery().is_none()
            && error.lifecycle.complete_failure.is_none()
            && error.lifecycle.ppocr_message.is_none()
        {
            // This marks only the primary outcome. append_lifecycle_failure still
            // records every cause and stdio observation with its own identity.
            let _ = error
                .diagnostics()
                .recorded_event()
                .set(*outcome.event_id());
        }
        self.append_lifecycle_failure(
            RuntimeLifecycleFailureStage::OperationCleanup,
            RuntimeLifecycleFailure::Host(error),
            links,
            Some(*outcome.event_id()),
        )
    }

    pub(super) fn append_connection_failure(
        &self,
        context: &ConnectionFailureContext,
        stage: ConnectionFailureStage,
        error: &RuntimeHostError,
    ) -> RuntimeHostResult<()> {
        if error.diagnostics().recorded_event().get().is_some()
            || error.projection().code == RuntimeErrorCode::LedgerFailure
        {
            return self.append_lifecycle_failure(
                RuntimeLifecycleFailureStage::ConnectionCleanup,
                RuntimeLifecycleFailure::Host(error),
                context.links.clone(),
                None,
            );
        }
        let diagnostic = if error.projection().code == RuntimeErrorCode::ProtocolInvalid {
            DiagnosticCode::RuntimeProtocolInvalid
        } else {
            DiagnosticCode::RuntimeDiagnostic
        };
        self.append_event_raw(
            if error.is_fatal() {
                EventSeverity::Fatal
            } else {
                EventSeverity::Error
            },
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            context.links.clone(),
            RuntimePayloadDraft::failed(
                diagnostic,
                stage.effect(),
                DiagnosticDetailDraft::new(
                    "runtime_connection",
                    stage.as_str(),
                    "local_ipc",
                    error.operation(),
                    format!(
                        "host_code={} fatal={} connection_id={} request_decoded={} observation={}",
                        error.code(),
                        error.is_fatal(),
                        context.connection_serial,
                        context.request_decoded,
                        serde_json::json!({
                            "owner_epoch": self.owner_epoch,
                            "runtime_pid": std::process::id(),
                            "clock": "process_instant",
                            "stages": &context.timing,
                        }),
                    ),
                    Sensitivity::Internal,
                ),
                AuditInput::new(),
            ),
        )
        .map_err(|_| ledger_error("append_runtime_connection_failure"))
        .and_then(|event| self.record_required_failure(error, &event, context.links.clone()))
    }
}

pub(super) fn append_runtime_start_event(
    ledger: &GlobalLedger,
    events: &RuntimeEvents,
    state_root: &Path,
    takeover: bool,
    device_diagnostic_mode: actingcommand_contract::DeviceDiagnosticMode,
) -> RuntimeHostResult<()> {
    let payload = RuntimePayloadDraft::start_with_device_diagnostics(
        takeover,
        device_diagnostic_mode,
        audit_path(state_root),
    );
    let draft = events.draft(
        EventSeverity::Info,
        EventSource::Runtime,
        OriginModule::Runtime,
        EventActor::Runtime,
        EventLinksDraft::default(),
        payload,
    )?;
    let draft = events.sanitize(draft)?;
    ledger
        .append(draft)
        .map(|_| ())
        .map_err(|_| ledger_error("append_runtime_start"))
}

/// The `runtime.instance_bound` payload of one registered instance: a pending discovery
/// binding carries the discovered facts without host and port.
pub(super) fn instance_bound_payload(instance: &RegisteredInstance) -> RuntimePayloadDraft {
    let endpoint = instance.bound_adb_endpoint();
    let discovered = instance
        .adb_endpoint
        .as_ref()
        .and_then(ResolvedInstanceEndpoint::discovered_binding);
    RuntimePayloadDraft::instance_bound(
        instance.instance_alias.clone(),
        instance.provenance,
        endpoint.map(|endpoint| endpoint.host().to_owned()),
        endpoint.map(ResolvedAdbEndpoint::port),
        endpoint.is_some_and(ResolvedAdbEndpoint::serial_configured),
        if discovered.is_some() {
            InstanceBindingSource::Discovered
        } else {
            InstanceBindingSource::Explicit
        },
        discovered.map(DiscoveredInstanceBinding::instance_index),
        discovered.map(|binding| binding.instance_name().to_owned()),
        discovered.map(|binding| binding.provider_version().to_owned()),
        AuditInput::new(),
    )
}

/// Records one `runtime.instance_bound` fact per registered instance, in instance_id order.
pub(super) fn append_instance_binding_events(
    ledger: &GlobalLedger,
    events: &RuntimeEvents,
    instances: &BTreeMap<InstanceId, RegisteredInstance>,
) -> RuntimeHostResult<()> {
    for instance in instances.values() {
        let links = events.system_links()?.with_instance_id(
            events
                .issuer()
                .issue_registered_instance(instance.instance_id),
        );
        let draft = events.draft(
            EventSeverity::Info,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            links,
            instance_bound_payload(instance),
        )?;
        let draft = events.sanitize(draft)?;
        ledger
            .append(draft)
            .map_err(|_| ledger_error("append_runtime_instance_bound"))?;
    }
    Ok(())
}

pub(super) fn record_failure(slot: &mut Option<RuntimeHostError>, result: RuntimeHostResult<()>) {
    if let Err(error) = result {
        *slot = Some(match slot.take() {
            Some(prior) => prior.with_complete_failure(
                crate::error::RuntimeFailureRelation::LifecycleRecord,
                error,
            ),
            None => error,
        });
    }
}
