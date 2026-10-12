// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use crate::codes::{HostCode, HostLocation};
use crate::recovery_read::{RECOVERY_PAGE_EVENTS, read_event_types};
use actingcommand_contract::codes::ContractLocation;
use actingcommand_contract::outcome::Location;

impl HostShared {
    pub(super) fn expire_agent_sessions(&self) -> RuntimeHostResult<()> {
        if self.agent_dispatcher_config.is_none() {
            return Ok(());
        }
        let now_unix_ms = unix_ms_now()?;
        let _gate = lock(&self.agent_write_gate, HostLocation::ExpireAgentSessions)?;
        let expired = lock(&self.agent_dispatcher, HostLocation::ExpireAgentSessions)?
            .expired_sessions(now_unix_ms)?;
        for data in expired {
            let links = self
                .events
                .issuer()
                .issue_agent_session_links(data.status().instance_id())
                .map_err(|_| runtime_identifier_error())?;
            let persisted = self.append_event_raw(
                EventSeverity::Warning,
                EventSource::System,
                OriginModule::AgentDispatcher,
                EventActor::Runtime,
                links.event_links(),
                AgentPayloadDraft::session_escalated(data, AuditInput::new()),
            )?;
            lock(&self.agent_dispatcher, HostLocation::CommitAgentTimeout)?
                .apply_event(&persisted, None)?;
        }
        Ok(())
    }

    pub(super) fn start_agent_session(
        &self,
        request: &RuntimeRequest,
        validated: &ValidatedRuntimeRequest<'_>,
        wake_id: AgentWakeId,
    ) -> Result<OperationSuccess, RequestFailure> {
        self.require_agent_dispatcher(ContractLocation::StartAgentSession)?;
        let _gate = lock(&self.agent_write_gate, ContractLocation::StartAgentSession)
            .map_err(RequestFailure::poison_without_terminal)?;
        let session_id = self
            .events
            .issuer()
            .mint_agent_session_id()
            .map(|issued| *issued.transport())
            .map_err(|_| RequestFailure::poison_without_terminal(runtime_identifier_error()))?;
        let started_at_unix_ms = unix_ms_now().map_err(RequestFailure::poison_without_terminal)?;
        let preparation = lock(&self.agent_dispatcher, ContractLocation::StartAgentSession)?
            .prepare_start(wake_id, session_id, started_at_unix_ms)
            .map_err(agent_request_failure)?;
        let (status, terminal) = match preparation {
            AgentSessionPreparation::Replay(status) => (status, None),
            AgentSessionPreparation::New(data) => {
                let persisted = self.append_event(
                    EventSeverity::Info,
                    request.source(),
                    OriginModule::AgentDispatcher,
                    request.actor(),
                    validated.event_links(Some(data.status().instance_id()), None, None),
                    AgentPayloadDraft::session_started(data.clone(), AuditInput::new()),
                )?;
                lock(
                    &self.agent_dispatcher,
                    HostLocation::CommitAgentSessionStart,
                )?
                .apply_event(&persisted, None)
                .map_err(RequestFailure::poison_without_terminal)?;
                (data.status().clone(), Some(terminal(&persisted)))
            }
        };
        let context = self.agent_session_context(status)?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal,
            result: RuntimeResult::AgentSessionOpened {
                context: Box::new(context),
            },
        })
    }

    pub(super) fn resume_agent_session(
        &self,
        request: &RuntimeRequest,
        validated: &ValidatedRuntimeRequest<'_>,
        session_id: AgentSessionId,
    ) -> Result<OperationSuccess, RequestFailure> {
        self.require_agent_dispatcher(ContractLocation::ResumeAgentSession)?;
        let _gate = lock(&self.agent_write_gate, ContractLocation::ResumeAgentSession)
            .map_err(RequestFailure::poison_without_terminal)?;
        let observed_at_unix_ms = unix_ms_now().map_err(RequestFailure::poison_without_terminal)?;
        let preparation = lock(&self.agent_dispatcher, ContractLocation::ResumeAgentSession)?
            .prepare_resume(
                request.request_id(),
                request.correlation_id(),
                session_id,
                observed_at_unix_ms,
            )
            .map_err(agent_request_failure)?;
        let (status, terminal) = match preparation {
            AgentResumePreparation::Replay { status, terminal } => (status, terminal),
            AgentResumePreparation::New(data) => {
                let persisted = self.append_event(
                    EventSeverity::Info,
                    request.source(),
                    OriginModule::AgentDispatcher,
                    request.actor(),
                    validated.event_links(Some(data.status().instance_id()), None, None),
                    AgentPayloadDraft::session_resumed(data.clone(), AuditInput::new()),
                )?;
                lock(
                    &self.agent_dispatcher,
                    HostLocation::CommitAgentSessionResume,
                )?
                .apply_event(&persisted, None)
                .map_err(RequestFailure::poison_without_terminal)?;
                (data.status().clone(), terminal(&persisted))
            }
        };
        let context = self.agent_session_context(status)?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: Some(terminal),
            result: RuntimeResult::AgentSessionObserved {
                context: Box::new(context),
            },
        })
    }

    pub(super) fn agent_session_status(
        &self,
        session_id: AgentSessionId,
    ) -> Result<OperationSuccess, RequestFailure> {
        self.require_agent_dispatcher(HostLocation::ReadAgentSession)?;
        let status = lock(&self.agent_dispatcher, HostLocation::ReadAgentSession)?
            .session(session_id)
            .map_err(agent_request_failure)?
            .clone();
        let context = self.agent_session_context(status)?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: None,
            result: RuntimeResult::AgentSessionObserved {
                context: Box::new(context),
            },
        })
    }

    pub(super) fn record_agent_response(
        &self,
        request: &RuntimeRequest,
        validated: &ValidatedRuntimeRequest<'_>,
        response: &AgentSessionResponse,
    ) -> Result<OperationSuccess, RequestFailure> {
        self.require_agent_dispatcher(ContractLocation::RecordAgentResponse)?;
        let _gate = lock(
            &self.agent_write_gate,
            ContractLocation::RecordAgentResponse,
        )
        .map_err(RequestFailure::poison_without_terminal)?;
        let observed_at_unix_ms = unix_ms_now().map_err(RequestFailure::poison_without_terminal)?;
        let preparation = lock(
            &self.agent_dispatcher,
            ContractLocation::RecordAgentResponse,
        )?
        .prepare_response(request.request_id(), response, observed_at_unix_ms)
        .map_err(agent_request_failure)?;
        let (data, payload, severity) = match preparation {
            AgentResponsePreparation::Replay(status) => {
                return Ok(OperationSuccess {
                    state: RuntimeReceiptState::Completed,
                    terminal: None,
                    result: RuntimeResult::AgentResponseRecorded { status },
                });
            }
            AgentResponsePreparation::Retry(data) => {
                let payload = AgentPayloadDraft::response_recorded(data.clone(), AuditInput::new());
                (data, payload, EventSeverity::Warning)
            }
            AgentResponsePreparation::Complete(data) => {
                let payload = AgentPayloadDraft::session_completed(data.clone(), AuditInput::new());
                (data, payload, EventSeverity::Info)
            }
            AgentResponsePreparation::Escalate(data) => {
                let payload = AgentPayloadDraft::session_escalated(data.clone(), AuditInput::new());
                (data, payload, EventSeverity::Warning)
            }
        };
        let persisted = self.append_event(
            severity,
            request.source(),
            OriginModule::AgentDispatcher,
            request.actor(),
            validated.event_links(Some(data.status().instance_id()), None, None),
            payload,
        )?;
        lock(&self.agent_dispatcher, HostLocation::CommitAgentResponse)?
            .apply_event(&persisted, None)
            .map_err(RequestFailure::poison_without_terminal)?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: Some(terminal(&persisted)),
            result: RuntimeResult::AgentResponseRecorded {
                status: data.status().clone(),
            },
        })
    }

    fn agent_session_context(
        &self,
        status: AgentSessionStatus,
    ) -> Result<AgentSessionContext, RequestFailure> {
        lock(&self.agent_dispatcher, HostLocation::ProjectAgentSession)?
            .context(&self.ledger, status)
            .map_err(agent_request_failure)
    }

    fn require_agent_dispatcher(
        &self,
        operation: impl Into<Location>,
    ) -> Result<(), RequestFailure> {
        let operation: Location = operation.into();
        self.agent_dispatcher_config
            .as_ref()
            .map(|_| ())
            .ok_or_else(|| {
                agent_request_failure(RuntimeHostError::request(
                    HostCode::AgentDispatcherDisabled,
                    operation,
                    RuntimeErrorCode::InvalidRequest,
                ))
            })
    }
}

pub(super) fn reconcile_agent_wakes(
    state: &mut AgentDispatcherState,
    ledger: &GlobalLedger,
    events: &RuntimeEvents,
    instances: &BTreeMap<InstanceId, RegisteredInstance>,
    config: &AgentDispatcherConfig,
) -> RuntimeHostResult<()> {
    // Workflow #381 R4a: the planning signals through one position, in bounded pages.
    let sources = ledger
        .latest_sequence()
        .and_then(|through| {
            read_event_types(
                ledger,
                &[EventType::PolicyPlanningSignalObserved],
                through,
                RECOVERY_PAGE_EVENTS,
            )
        })
        .map_err(|_| ledger_error(HostLocation::QueryAgentWakeSources))?;
    for source in sources {
        if state.has_wake_for_trigger(source.event_id()) {
            continue;
        }
        let EventPayload::Policy(actingcommand_contract::PolicyPayload::PlanningSignalObserved(
            signal,
        )) = source.payload()
        else {
            return Err(RuntimeHostError::fatal(
                HostCode::AgentWakeSourceInvalid,
                HostLocation::ReconcileAgentWakes,
                RuntimeErrorCode::RuntimeFatal,
            ));
        };
        let kind = match signal.kind() {
            actingcommand_contract::PolicyPlanningSignalKind::TimelineReached => {
                AgentWakeKind::TimelineReached
            }
            actingcommand_contract::PolicyPlanningSignalKind::DriftPredicted => {
                AgentWakeKind::DriftPredicted
            }
            _ => continue,
        };
        let instance_id = instances
            .values()
            .find(|instance| instance.instance_alias == signal.instance_id())
            .map(|instance| instance.instance_id)
            .ok_or_else(|| {
                RuntimeHostError::fatal(
                    HostCode::AgentWakeInstanceUnknown,
                    HostLocation::ReconcileAgentWakes,
                    RuntimeErrorCode::RuntimeFatal,
                )
            })?;
        append_agent_wake(state, ledger, events, config, &source, instance_id, kind)?;
    }
    Ok(())
}

pub(super) fn append_agent_wake(
    state: &mut AgentDispatcherState,
    ledger: &GlobalLedger,
    events: &RuntimeEvents,
    config: &AgentDispatcherConfig,
    source: &PersistedEvent,
    instance_id: InstanceId,
    kind: AgentWakeKind,
) -> RuntimeHostResult<PersistedEvent> {
    let issued = events
        .issuer()
        .issue_agent_wake(
            AgentWakeTrigger::new(
                instance_id,
                kind,
                *source.event_id(),
                source.sequence(),
                source.timestamp_unix_ms(),
            )
            .map_err(|_| {
                RuntimeHostError::fatal(
                    HostCode::AgentWakeTriggerInvalid,
                    HostLocation::AppendAgentWake,
                    RuntimeErrorCode::RuntimeFatal,
                )
            })?,
            config.budget(),
            config.capabilities().clone(),
        )
        .map_err(|_| {
            RuntimeHostError::fatal(
                HostCode::AgentWakeIssueFailed,
                HostLocation::AppendAgentWake,
                RuntimeErrorCode::RuntimeFatal,
            )
        })?;
    let draft = events.draft(
        EventSeverity::Info,
        EventSource::Scheduler,
        OriginModule::AgentDispatcher,
        EventActor::Runtime,
        issued.event_links(),
        AgentPayloadDraft::wake_requested(issued.data().clone(), AuditInput::new()),
    )?;
    let persisted = ledger
        .append(events.sanitize(draft)?)
        .map_err(|_| ledger_error(HostLocation::AppendAgentWake))?;
    state.apply_event(&persisted, None)?;
    Ok(persisted)
}

fn agent_request_failure(error: RuntimeHostError) -> RequestFailure {
    if error.is_fatal() {
        RequestFailure::poison_without_terminal(error)
    } else {
        RequestFailure::request(error, RuntimeReceiptState::Denied, None)
    }
}
