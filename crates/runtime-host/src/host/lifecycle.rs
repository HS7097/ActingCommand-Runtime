// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use crate::codes::HostLocation;
use actingcommand_contract::outcome::{Location, Outcome};

/// Each lifecycle failure stage: its registered location (ledger revision 3: the `stage` of the
/// failure's outcome) and the dotted token it had before, which the record's diagnostic detail
/// keeps as its stage (Workflow #378 A3). The last two are stages a host error names itself.
const LIFECYCLE_STAGES: [(HostLocation, &str); 20] = [
    // Workflow #381 A: a lifecycle failure stage had 2-4 dotted segments; the former one-word
    // install stages failed that check, so no install failure could be recorded and every
    // attempt latched the lifecycle append failure.
    (
        HostLocation::RuntimeLifecycleInstallDrain,
        "runtime.lifecycle.install_drain",
    ),
    (
        HostLocation::RuntimeLifecycleInstallHeld,
        "runtime.lifecycle.install_held",
    ),
    (
        HostLocation::RuntimeLifecycleInstallRelease,
        "runtime.lifecycle.install_release",
    ),
    (
        HostLocation::RuntimeLifecyclePolicyInitialization,
        "runtime.lifecycle.policy_initialization",
    ),
    (
        HostLocation::RuntimeLifecyclePolicyMonitor,
        "runtime.lifecycle.policy_monitor",
    ),
    (
        HostLocation::RuntimeLifecyclePolicyForward,
        "runtime.lifecycle.policy_forward",
    ),
    (
        HostLocation::RuntimeLifecycleStrategicReport,
        "runtime.lifecycle.strategic_report",
    ),
    (
        HostLocation::RuntimeLifecycleSessionClose,
        "runtime.lifecycle.session_close",
    ),
    (
        HostLocation::RuntimeLifecycleConnectionPreparation,
        "runtime.lifecycle.connection_preparation",
    ),
    (
        HostLocation::RuntimeLifecycleOperationCleanup,
        "runtime.lifecycle.operation_cleanup",
    ),
    (
        HostLocation::RuntimeLifecycleConnectionCleanup,
        "runtime.lifecycle.connection_cleanup",
    ),
    (
        HostLocation::RuntimeLifecycleShutdownJoin,
        "runtime.lifecycle.shutdown_join",
    ),
    (
        HostLocation::RuntimeLifecycleInfoFileRemoval,
        "runtime.lifecycle.info_file_removal",
    ),
    (
        HostLocation::RuntimeLifecycleRetainedReference,
        "runtime.lifecycle.retained_reference",
    ),
    (
        HostLocation::RuntimeLifecycleHostClose,
        "runtime.lifecycle.host_close",
    ),
    (
        HostLocation::RuntimeLifecyclePolicyDriver,
        "runtime.lifecycle.policy_driver",
    ),
    (
        HostLocation::RuntimeLifecyclePolicyControl,
        "runtime.lifecycle.policy_control",
    ),
    (
        HostLocation::RuntimeLifecyclePolicyBootstrap,
        "runtime.lifecycle.policy_bootstrap",
    ),
    (
        HostLocation::RuntimeLifecycleSelectionRecord,
        "runtime.lifecycle.selection_record",
    ),
    (
        HostLocation::RuntimeLifecycleShutdownRequested,
        "runtime.lifecycle.shutdown_requested",
    ),
];

/// The diagnostic detail's stage token of a lifecycle failure stage: its former dotted
/// spelling, else the location's own spelling (a detail stage takes 1-4 dotted segments).
fn lifecycle_detail_stage(stage: Location) -> &'static str {
    LIFECYCLE_STAGES
        .iter()
        .find(|(location, _)| location.location() == stage)
        .map_or(stage.as_str(), |(_, token)| *token)
}

impl RuntimeLifecycleFailureStage {
    /// The stage's registered location: the `stage` of its lifecycle failure's outcome.
    pub(super) const fn location(self) -> HostLocation {
        match self {
            Self::InstallDrain => HostLocation::RuntimeLifecycleInstallDrain,
            Self::InstallHeld => HostLocation::RuntimeLifecycleInstallHeld,
            Self::InstallRelease => HostLocation::RuntimeLifecycleInstallRelease,
            Self::PolicyInitialization => HostLocation::RuntimeLifecyclePolicyInitialization,
            Self::PolicyMonitor => HostLocation::RuntimeLifecyclePolicyMonitor,
            Self::PolicyForward => HostLocation::RuntimeLifecyclePolicyForward,
            Self::StrategicReport => HostLocation::RuntimeLifecycleStrategicReport,
            Self::SessionClose => HostLocation::RuntimeLifecycleSessionClose,
            Self::ConnectionPreparation => HostLocation::RuntimeLifecycleConnectionPreparation,
            Self::OperationCleanup => HostLocation::RuntimeLifecycleOperationCleanup,
            Self::ConnectionCleanup => HostLocation::RuntimeLifecycleConnectionCleanup,
            Self::ShutdownJoin => HostLocation::RuntimeLifecycleShutdownJoin,
            Self::InfoFileRemoval => HostLocation::RuntimeLifecycleInfoFileRemoval,
            Self::RetainedReference => HostLocation::RuntimeLifecycleRetainedReference,
            Self::HostClose => HostLocation::RuntimeLifecycleHostClose,
            Self::PolicyDriver => HostLocation::RuntimeLifecyclePolicyDriver,
            Self::PolicyControl => HostLocation::RuntimeLifecyclePolicyControl,
            Self::PolicyBootstrap => HostLocation::RuntimeLifecyclePolicyBootstrap,
        }
    }

    /// The stage's former dotted spelling, which diagnostic details keep as their stage token.
    pub(super) fn as_str(self) -> &'static str {
        lifecycle_detail_stage(self.location().location())
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
        .map_err(|_| ledger_error(HostLocation::AppendRuntimeLifecycleObserved))
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
            return Err(ledger_error(HostLocation::AppendVendorStdioClose));
        }
        let gate = lock(&self.fact_write_gate, HostLocation::AppendVendorStdioClose)?;
        if self.lifecycle_append_failed.load(Ordering::Acquire) {
            return Err(ledger_error(HostLocation::AppendVendorStdioClose));
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
                    ledger_error(HostLocation::AppendVendorStdioClose)
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
        self.append_lifecycle_failure_with_severity(stage, failure, links, entered_event_id, None)
    }

    /// `append_lifecycle_failure` whose primary record of a non-fatal failure requests
    /// `primary_severity` instead of `Error`; a cause record and a fatal record keep theirs
    /// (Workflow #336 L2c: a `linear_steps` task failure's record takes its terminal's severity).
    /// Ledger revision 3 derives every record's severity from its outcome's category, so the
    /// record and the terminal of the same failure agree; only a `Debug` request is kept.
    pub(super) fn append_lifecycle_failure_with_severity(
        &self,
        stage: RuntimeLifecycleFailureStage,
        failure: RuntimeLifecycleFailure<'_>,
        links: EventLinksDraft,
        entered_event_id: Option<EventId>,
        primary_severity: Option<EventSeverity>,
    ) -> RuntimeHostResult<()> {
        let decision_id = match &failure {
            RuntimeLifecycleFailure::PolicyAdmission { decision_id, .. } => Some(*decision_id),
            _ => None,
        };
        let host_error = match &failure {
            RuntimeLifecycleFailure::Host(error)
            | RuntimeLifecycleFailure::PolicyAdmission { error, .. } => Some(*error),
            _ => None,
        };
        let failure_stage = host_error
            .and_then(|error| error.lifecycle.failure_stage)
            .unwrap_or_else(|| stage.location().location());
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
            return Err(ledger_error(HostLocation::AppendRuntimeLifecycleFailure));
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
        // Ledger revision 3 (Workflow #378 A3): the failure is the record's outcome, built from
        // the failing error or from the client's or actingd's own code, with the registered
        // stage; no Runtime text (the former JSON summary, the client's display) is recorded.
        let (origin, outcome, operation, fatal, runtime_code) = match failure {
            RuntimeLifecycleFailure::Host(error)
            | RuntimeLifecycleFailure::PolicyAdmission { error, .. } => (
                "runtime_host",
                error.outcome(),
                error.operation(),
                error.is_fatal(),
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
                Outcome::new(code).operation(operation),
                operation.as_str(),
                fatal,
                runtime_code,
            ),
            RuntimeLifecycleFailure::Process { code } => (
                "actingd",
                Outcome::relayed(code),
                "actingd_process",
                false,
                None,
            ),
        };
        let outcome = outcome.stage(failure_stage);
        let detail_stage = lifecycle_detail_stage(failure_stage);
        let gate = lock(
            &self.fact_write_gate,
            HostLocation::AppendRuntimeLifecycleFailure,
        )?;
        let mut persisted = Vec::new();
        let mut emit = |cause: Option<&actingcommand_contract::LifecycleCauseDraft>,
                        reference: Option<EventId>| {
            let lifecycle = actingcommand_contract::RuntimeLifecycleFailureDraft::new(
                self.owner_epoch,
                origin,
                outcome.clone(),
            )
            .with_runtime_code(runtime_code)
            .with_entered_event_id(reference.or_else(|| {
                host_error.and_then(|error| error.diagnostics().prior_recorded_event())
            }))
            .with_instance_id(host_error.and_then(|error| error.lifecycle.instance_id))
            .with_resource_dispositions(if cause.is_none() {
                host_error.and_then(|error| error.diagnostics().resource_dispositions())
            } else {
                None
            })
            .with_primary_detail(host_error.and_then(|error| error.diagnostic_detail().cloned()))
            .with_adb_recovery(
                host_error.and_then(|error| error.diagnostics().adb_recovery().cloned()),
            )
            .with_native_detail(
                host_error.and_then(|error| error.diagnostics().native_detail().cloned()),
            )
            .with_capacity(host_error.and_then(|error| error.lifecycle.capacity.clone()))
            .with_task_timing(host_error.and_then(|error| error.lifecycle.task_timing.clone()))
            .with_raw_os_error(host_error.and_then(|error| error.lifecycle.raw_os_error))
            .with_cleanup_cause(host_error.and_then(|error| error.cleanup_cause().cloned()))
            .with_cause(cause.cloned());
            // The record's severity is its outcome's category (ledger revision 3); this
            // requested severity is kept only as the producer's choice of a non-debug record.
            let cause_fatal = cause.map_or(fatal, |cause| {
                cause.severity() == actingcommand_contract::CleanupCauseSeverity::Fatal
            });
            let event = self
                .append_event_under_fact_gate(
                    if cause_fatal {
                        EventSeverity::Fatal
                    } else {
                        primary_severity
                            .filter(|_| cause.is_none())
                            .unwrap_or(EventSeverity::Error)
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
                        host_error
                            .and_then(|error| error.lifecycle.installation_effect)
                            .unwrap_or(if decision_id.is_some() {
                                EffectDisposition::NotPerformed
                            } else {
                                EffectDisposition::Indeterminate
                            }),
                        DiagnosticDetailDraft::new(
                            "runtime_lifecycle",
                            detail_stage,
                            origin,
                            operation,
                        ),
                        lifecycle,
                        AuditInput::new(),
                    ),
                )
                .map_err(|_| {
                    self.lifecycle_append_failed.store(true, Ordering::Release);
                    ledger_error(HostLocation::AppendRuntimeLifecycleFailure)
                })?;
            let id = *event.event_id();
            persisted.push(event);
            Ok::<_, RuntimeHostError>(id)
        };
        if let Some(error) = host_error {
            let phase_close = crate::error::spelled(
                error.code(),
                &[
                    actingcommand_execution_kernel::ExecutionKernelCode::InputBackendCloseFailed
                        .code(),
                    actingcommand_execution_kernel::ExecutionKernelCode::CaptureBackendCloseFailed
                        .code(),
                ],
            ) && error.diagnostics().resource_dispositions().is_none()
                && !error.diagnostics().is_resource_close_recording()
                && error.diagnostics().lifecycle_causes().iter().any(|cause| {
                    cause.cause.phase() != actingcommand_contract::LifecycleFailurePhase::Retirement
                });
            if error.diagnostics().recorded_event().get().is_none() && !phase_close {
                // Ledger revision 3: the PPOCR display text (Runtime text) is not recorded, so
                // the failure is one record, not one per 1 KiB part of that text.
                let id = emit(None, entered_event_id)?;
                let _ = error.diagnostics().recorded_event().set(id);
            }
            let reference =
                entered_event_id.or_else(|| error.diagnostics().recorded_event().get().copied());
            for cause in error.diagnostics().lifecycle_causes() {
                if cause.recorded_event.get().is_none() {
                    let id = emit(Some(&cause.cause), reference)?;
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
        } else {
            // A client's or actingd's failure: one record of its code. Ledger revision 3 does
            // not record the client's display (Runtime text).
            emit(None, entered_event_id)?;
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
            let error = error.with_failure_stage(stage.location());
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
                    append_error.with_failure_stage(stage.location()),
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
        self.record_required_failure_with_severity(error, outcome, links, None)
    }

    /// `record_required_failure` whose lifecycle record, when one is written for a non-fatal
    /// error, takes `severity` instead of `Error` (Workflow #336 L2c: a `linear_steps` task
    /// failure's record takes the severity of the task terminal it names).
    pub(super) fn record_required_failure_with_severity(
        &self,
        error: &RuntimeHostError,
        outcome: &PersistedEvent,
        links: EventLinksDraft,
        severity: Option<EventSeverity>,
    ) -> RuntimeHostResult<()> {
        // These original failure builders copy this error's primary and cleanup
        // details into their outcome. Extra native details still need lifecycle facts.
        let (primary_detail_recorded, cleanup_cause_recorded) = match outcome.payload() {
            EventPayload::Capture(CapturePayload::Failed(payload))
            | EventPayload::Input(InputPayload::Failed(payload)) => (
                payload.detail().is_some(),
                // Ledger revision 3: a recorded cleanup cause is the outcome's `cleanup` link.
                payload.outcome().causes().iter().any(|link| {
                    *link.relation() == actingcommand_contract::outcome::CauseRelation::Cleanup
                }),
            ),
            _ => (false, false),
        };
        // Ledger revision 3: the earlier record carries this error's outcome when its own
        // outcome has the error's code with every value and every link of it: a capture or
        // input failure built from `record_outcome()`, a `runtime.failed` diagnostic of it, a
        // policy rejection. A record of another code (a diagnostic code alone) does not, so an
        // error whose outcome carries a `caused_by` link or values gets its lifecycle record.
        let error_outcome = error.outcome();
        let outcome_recorded = outcome.payload().outcome().is_some_and(|recorded| {
            recorded.code() == error_outcome.code()
                && error_outcome
                    .values()
                    .iter()
                    .all(|(key, value)| recorded.value(key) == Some(value))
                && error_outcome
                    .causes()
                    .iter()
                    .all(|link| recorded.causes().contains(link))
        });
        // Ledger revision 3: the device message of a failure without a fixed summary was the
        // primary detail's message before; the failure's outcome now carries it as a foreign
        // link, so like the detail it needs no lifecycle fact of its own.
        let native_detail_recorded = error.diagnostics().native_detail().is_none()
            || (primary_detail_recorded && error.diagnostics().native_detail_is_device_message());
        if native_detail_recorded
            && (outcome_recorded
                || (error.lifecycle.caused_by.is_none() && !error.has_outcome_values()))
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
        self.append_lifecycle_failure_with_severity(
            RuntimeLifecycleFailureStage::OperationCleanup,
            RuntimeLifecycleFailure::Host(error),
            links,
            Some(*outcome.event_id()),
            severity,
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
            // Ledger revision 3: the failure is the record's outcome; the former `key=value`
            // summary and the connection's timing observation (Runtime text) are not recorded.
            RuntimePayloadDraft::failed(
                diagnostic,
                stage.effect(),
                error.record_outcome(),
                DiagnosticDetailDraft::new(
                    "runtime_connection",
                    stage.as_str(),
                    "local_ipc",
                    error.operation(),
                ),
                AuditInput::new(),
            ),
        )
        .map_err(|_| ledger_error(HostLocation::AppendRuntimeConnectionFailure))
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
        .map_err(|_| ledger_error(HostLocation::AppendRuntimeStart))
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
            .map_err(|_| ledger_error(HostLocation::AppendRuntimeInstanceBound))?;
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
